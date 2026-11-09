//! Prepared against the real Auth bridge; execution awaits the composed S2 GO.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, with_authenticated_control_recovery_for_test},
    audit::Store,
    model::Event,
    policy::Policy,
    redaction::Redactor,
};
use axum::{extract::State, http::header, routing::get};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Mutex, mpsc},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RUNNER: &str = "fixture-gateway-lifecycle-tenant-a-runner-20261108";
const VIEWER: &str = "fixture-gateway-lifecycle-tenant-a-viewer-20261108";
const TEST_LIMIT: Duration = Duration::from_secs(15);

fn fixture() -> (tempfile::TempDir, GatewayState, Arc<Store>) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Arc::new(
        Store::open_for_tenant(
            &directory.path().join("tenant.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: Arc::clone(&store),
            runner_token: RUNNER.into(),
            viewer_token: VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    (directory, state, store)
}

fn headers() -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {RUNNER}").parse().unwrap(),
    );
    headers
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(TEST_LIMIT, async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn port_closed(address: SocketAddr) {
    tokio::time::timeout(TEST_LIMIT, async {
        loop {
            match tokio::net::TcpStream::connect(address).await {
                Ok(connection) => drop(connection),
                Err(_) => return,
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn request_get(address: SocketAddr, path: &str) -> String {
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    connection
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    read_to_close(&mut connection, &mut response).await;
    String::from_utf8(response).unwrap()
}

fn epoch_router(state: GatewayState) -> Router {
    Router::new()
        .route(
            "/epoch",
            get(|State(state): State<GatewayState>| async move {
                with_authenticated_control_recovery_for_test(&state, headers(), |auth, store| {
                    let _usage = auth.acquire_usage(&store)?;
                    Ok(auth.auth_epoch_id().to_string())
                })
                .await
            }),
        )
        .route(
            "/stream",
            get(|| async {
                use axum::response::sse::{Event, Sse};
                let first = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(
                        Event::default().data("lifecycle-stream-open"),
                    )
                });
                Sse::new(first.chain(futures_util::stream::pending()))
            }),
        )
        .route("/owner-panic", get(connection_owner_failure))
        .with_state(state)
}

async fn connection_owner_failure() {
    // A panic in this test-owned request task becomes the real JoinSet
    // connection-owner failure; never a production endpoint.
    panic!("synthetic gateway connection-owner failure");
}

async fn open_stream(address: SocketAddr) -> tokio::net::TcpStream {
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    connection
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(TEST_LIMIT, async {
        let mut byte = [0];
        while !response.ends_with(b"lifecycle-stream-open\n\n\r\n") {
            assert_ne!(connection.read(&mut byte).await.unwrap(), 0);
            response.push(byte[0]);
        }
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    connection
}

async fn stream_closed(mut connection: tokio::net::TcpStream) {
    let mut remaining = Vec::new();
    read_to_close(&mut connection, &mut remaining).await;
}

async fn stream_survives_initial_grace(connection: &mut tokio::net::TcpStream) {
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), connection.read(&mut byte))
            .await
            .is_err(),
        "graceful shutdown must not immediately cut an established SSE stream"
    );
}

async fn read_to_close(connection: &mut tokio::net::TcpStream, output: &mut Vec<u8>) {
    if let Err(error) = tokio::time::timeout(TEST_LIMIT, connection.read_to_end(output))
        .await
        .unwrap()
    {
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        ));
    }
}

fn assert_reopened_commit(directory: &tempfile::TempDir, store: &Store) {
    let reopened = Store::open_existing_for_tenant(
        &directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    assert_eq!(reopened.head().unwrap(), store.head().unwrap());
    assert!(reopened.integrity().unwrap().valid);
}

#[tokio::test]
async fn dropping_the_owner_before_signal_setup_or_binding_retires_the_real_epoch() {
    let (_directory, state, _store) = fixture();
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(), |auth, store| {
            let _usage = auth.acquire_usage(&store)?;
            Ok(())
        })
        .await
        .is_ok()
    );
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    drop(owner);
    assert!(retirement.drained());
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(), |_, _| Ok(()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn both_real_listener_surfaces_authenticate_in_the_same_epoch() {
    let (_directory, state, _store) = fixture();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (stop, signal) = tokio::sync::oneshot::channel();
    let (bound, addresses) = tokio::sync::oneshot::channel();
    let public = epoch_router(state.clone());
    let control = epoch_router(state);
    let task = tokio::spawn(owner.serve(
        ("127.0.0.1:0".parse().unwrap(), public),
        Some(("127.0.0.1:0".parse().unwrap(), control)),
        async {
            let _ = signal.await;
        },
        move |public, control| {
            bound.send((public, control.unwrap())).unwrap();
            Ok(())
        },
    ));
    let (public, control) = addresses.await.unwrap();
    let public_response = request_get(public, "/epoch").await;
    let control_response = request_get(control, "/epoch").await;
    assert!(public_response.starts_with("HTTP/1.1 200"));
    assert!(control_response.starts_with("HTTP/1.1 200"));
    let public_epoch = public_response.split("\r\n\r\n").nth(1).unwrap();
    let control_epoch = control_response.split("\r\n\r\n").nth(1).unwrap();
    assert_eq!(public_epoch, control_epoch);
    assert!(!retirement.drained());
    stop.send(()).unwrap();
    tokio::time::timeout(TEST_LIMIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(retirement.drained());
    port_closed(public).await;
    port_closed(control).await;
}

#[tokio::test]
async fn occupied_first_or_second_bind_retires_and_leaves_no_new_listener() {
    for occupied_first in [true, false] {
        let (_directory, state, _store) = fixture();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_address = occupied.local_addr().unwrap();
        let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let free_address = free.local_addr().unwrap();
        drop(free);
        let owner = GatewayServer::new(state.control_recovery_retirement());
        let retirement = Arc::clone(&owner.retirement);
        let (public, control) = if occupied_first {
            (occupied_address, free_address)
        } else {
            (free_address, occupied_address)
        };
        assert!(
            owner
                .serve(
                    (public, Router::new()),
                    Some((control, Router::new())),
                    std::future::pending(),
                    |_, _| panic!("a failed bind must not publish a ready server"),
                )
                .await
                .is_err()
        );
        assert!(retirement.drained());
        assert!(
            with_authenticated_control_recovery_for_test(&state, headers(), |_, _| Ok(()))
                .await
                .is_err()
        );
        // The occupied fixture is retained; only the newly bound surface closes.
        port_closed(free_address).await;
        assert!(
            tokio::net::TcpStream::connect(occupied_address)
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn malformed_peer_connection_does_not_retire_the_gateway_epoch() {
    let (_directory, state, _store) = fixture();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (stop, signal) = tokio::sync::oneshot::channel();
    let (bound, address) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(owner.serve(
        ("127.0.0.1:0".parse().unwrap(), epoch_router(state.clone())),
        None,
        async {
            let _ = signal.await;
        },
        move |public, control| {
            assert!(control.is_none());
            bound.send(public).unwrap();
            Ok(())
        },
    ));
    let address = address.await.unwrap();
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    let malformed = b"GET /epoch HTTP/1.1\r\ninvalid header\r\n\r\n";
    assert!(malformed.len() <= 1024);
    connection.write_all(malformed).await.unwrap();
    let mut response = Vec::new();
    read_to_close(&mut connection, &mut response).await;
    assert!(response.starts_with(b"HTTP/1.1 400"));
    assert!(
        request_get(address, "/epoch")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(), |auth, store| {
            let _usage = auth.acquire_usage(&store)?;
            Ok(())
        })
        .await
        .is_ok()
    );
    assert!(!task.is_finished());
    stop.send(()).unwrap();
    tokio::time::timeout(TEST_LIMIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(retirement.drained());
    port_closed(address).await;
}

#[derive(Clone)]
struct HeldUsage {
    state: GatewayState,
    entered: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
}

fn held_usage_router(
    state: GatewayState,
) -> (Router, tokio::sync::oneshot::Receiver<()>, mpsc::Sender<()>) {
    let (entered, acquired) = tokio::sync::oneshot::channel();
    let (release, resume) = mpsc::channel();
    let state = HeldUsage {
        state,
        entered: Arc::new(Mutex::new(Some(entered))),
        release: Arc::new(Mutex::new(Some(resume))),
    };
    let router = Router::new()
        .route(
            "/held",
            get(|State(held): State<HeldUsage>| async move {
                with_authenticated_control_recovery_for_test(
                    &held.state,
                    headers(),
                    move |auth, store| {
                        let _usage = auth.acquire_usage(&store)?;
                        held.entered
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(())
                            .unwrap();
                        held.release
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .recv_timeout(TEST_LIMIT)?;
                        // Real SQLite commit while the genuine Usage is still alive.
                        // This is an owner/drain probe, not a Core Close mutation proof.
                        let committed =
                            store.append(Event::new("fixture.gateway.lifecycle.committed"))?;
                        Ok(committed.hash)
                    },
                )
                .await
            }),
        )
        .with_state(state);
    (router, acquired, release)
}

#[tokio::test]
async fn signal_stops_both_acceptors_and_waits_for_real_usage_and_sqlite_commit() {
    // A five-second HTTP timeout or early lease release cannot satisfy this.
    let (directory, state, store) = fixture();
    let before = store.head().unwrap();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (router, acquired, release) = held_usage_router(state.clone());
    let (stop, signal) = tokio::sync::oneshot::channel();
    let (bound, addresses) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(owner.serve(
        ("127.0.0.1:0".parse().unwrap(), epoch_router(state.clone())),
        Some(("127.0.0.1:0".parse().unwrap(), router)),
        async {
            let _ = signal.await;
        },
        move |public, control| {
            bound.send((public, control.unwrap())).unwrap();
            Ok(())
        },
    ));
    let (public, control) = addresses.await.unwrap();
    let mut stream = open_stream(public).await;
    let request = tokio::spawn(async move { request_get(control, "/held").await });
    tokio::time::timeout(TEST_LIMIT, acquired)
        .await
        .unwrap()
        .unwrap();
    stop.send(()).unwrap();
    // The real factory proves retirement, not only a shutdown flag.
    tokio::time::timeout(TEST_LIMIT, async {
        loop {
            if with_authenticated_control_recovery_for_test(&state, headers(), |_, _| Ok(()))
                .await
                .is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    port_closed(public).await;
    port_closed(control).await;
    // These are real accepted TCP connections, held through the graceful5s.
    // Both must close even though the blocking worker still owns its Usage.
    stream_survives_initial_grace(&mut stream).await;
    stream_closed(stream).await;
    assert!(request.await.unwrap().is_empty());
    assert!(!retirement.drained());
    assert!(!task.is_finished());
    assert_eq!(store.head().unwrap(), before);
    release.send(()).unwrap();
    tokio::time::timeout(TEST_LIMIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(retirement.drained());
    assert_ne!(store.head().unwrap(), before);
    assert!(store.integrity().unwrap().valid);
    assert_reopened_commit(&directory, &store);
}

#[tokio::test]
async fn caller_cancellation_retires_then_owns_cleanup_until_the_blocking_commit_drains() {
    let (directory, state, store) = fixture();
    let before = store.head().unwrap();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (router, acquired, release) = held_usage_router(state.clone());
    let (bound, addresses) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(owner.serve(
        ("127.0.0.1:0".parse().unwrap(), router),
        Some(("127.0.0.1:0".parse().unwrap(), epoch_router(state.clone()))),
        std::future::pending(),
        move |public, control| {
            bound.send((public, control.unwrap())).unwrap();
            Ok(())
        },
    ));
    let (public, control) = addresses.await.unwrap();
    let mut stream = open_stream(control).await;
    let request = tokio::spawn(async move { request_get(public, "/held").await });
    tokio::time::timeout(TEST_LIMIT, acquired)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(), |_, _| Ok(()))
            .await
            .is_err()
    );
    port_closed(public).await;
    port_closed(control).await;
    stream_survives_initial_grace(&mut stream).await;
    stream_closed(stream).await;
    assert!(request.await.unwrap().is_empty());
    assert!(!retirement.drained());
    assert_eq!(store.head().unwrap(), before);
    release.send(()).unwrap();
    eventually(|| retirement.drained()).await;
    // Only this observer remains: the cleanup task released its real owner.
    eventually(|| Arc::strong_count(&retirement) == 1).await;
    assert_ne!(store.head().unwrap(), before);
    assert!(store.integrity().unwrap().valid);
    assert_reopened_commit(&directory, &store);
}

#[tokio::test]
async fn serving_owner_error_retires_before_draining_its_real_sibling_usage() {
    let (directory, state, store) = fixture();
    let before = store.head().unwrap();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (router, acquired, release) = held_usage_router(state.clone());
    let (bound, addresses) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(owner.serve(
        ("127.0.0.1:0".parse().unwrap(), epoch_router(state.clone())),
        Some(("127.0.0.1:0".parse().unwrap(), router)),
        std::future::pending(),
        move |public, control| {
            bound.send((public, control.unwrap())).unwrap();
            Ok(())
        },
    ));
    let (public, control) = addresses.await.unwrap();
    let mut stream = open_stream(public).await;
    let request = tokio::spawn(async move { request_get(control, "/held").await });
    tokio::time::timeout(TEST_LIMIT, acquired)
        .await
        .unwrap()
        .unwrap();
    assert!(request_get(public, "/owner-panic").await.is_empty());
    port_closed(public).await;
    port_closed(control).await;
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(), |_, _| Ok(()))
            .await
            .is_err()
    );
    assert!(!retirement.drained());
    stream_survives_initial_grace(&mut stream).await;
    stream_closed(stream).await;
    assert!(request.await.unwrap().is_empty());
    assert!(!retirement.drained());
    assert!(!task.is_finished());
    assert_eq!(store.head().unwrap(), before);
    release.send(()).unwrap();
    assert!(
        tokio::time::timeout(TEST_LIMIT, task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(retirement.drained());
    assert_ne!(store.head().unwrap(), before);
    assert_reopened_commit(&directory, &store);
}

#[derive(Clone)]
struct HeldPanic {
    entered: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    release: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
}

fn held_panic_router() -> (
    Router,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (entered, pending) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    let held = HeldPanic {
        entered: Arc::new(Mutex::new(Some(entered))),
        release: Arc::new(Mutex::new(Some(resume))),
    };
    (
        Router::new()
            .route("/held-panic", get(panic_after_release))
            .with_state(held),
        pending,
        release,
    )
}

async fn panic_after_release(State(held): State<HeldPanic>) {
    let resume = held.release.lock().unwrap().take().unwrap();
    held.entered
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(())
        .unwrap();
    tokio::time::timeout(TEST_LIMIT, resume)
        .await
        .unwrap()
        .unwrap();
    panic!("synthetic gateway connection-owner failure during graceful drain");
}

#[tokio::test]
async fn panic_during_grace_returns_error_only_after_connections_and_real_usage_drain() {
    // Lifecycle B1: r2 discards the JoinError from its post-signal drain.
    // This is separate from the historical Core B1 worker-quota regression.
    let (directory, state, store) = fixture();
    let before = store.head().unwrap();
    let owner = GatewayServer::new(state.control_recovery_retirement());
    let retirement = Arc::clone(&owner.retirement);
    let (usage_router, acquired, release_usage) = held_usage_router(state.clone());
    let (panic_router, entered, release_panic) = held_panic_router();
    let (stop, signal) = tokio::sync::oneshot::channel();
    let (bound, addresses) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(owner.serve(
        (
            "127.0.0.1:0".parse().unwrap(),
            epoch_router(state.clone()).merge(panic_router),
        ),
        Some(("127.0.0.1:0".parse().unwrap(), usage_router)),
        async {
            let _ = signal.await;
        },
        move |public, control| {
            bound.send((public, control.unwrap())).unwrap();
            Ok(())
        },
    ));
    let (public, control) = addresses.await.unwrap();
    let mut stream = open_stream(public).await;
    let panic_request = tokio::spawn(async move { request_get(public, "/held-panic").await });
    tokio::time::timeout(TEST_LIMIT, entered)
        .await
        .unwrap()
        .unwrap();
    let usage_request = tokio::spawn(async move { request_get(control, "/held").await });
    tokio::time::timeout(TEST_LIMIT, acquired)
        .await
        .unwrap()
        .unwrap();

    stop.send(()).unwrap();
    tokio::time::timeout(TEST_LIMIT, async {
        loop {
            if with_authenticated_control_recovery_for_test(&state, headers(), |auth, store| {
                let _usage = auth.acquire_usage(&store)?;
                Ok(())
            })
            .await
            .is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Closing both actual acceptors places the later panic after the owners
    // left their accept loops, so it must be classified by the drain path.
    port_closed(public).await;
    port_closed(control).await;
    assert!(!retirement.drained());
    assert!(!task.is_finished());
    stream_survives_initial_grace(&mut stream).await;
    release_panic.send(()).unwrap();
    // The handler connection closes while the separate SSE connection is
    // still alive. This proves completion during grace, rather than a flag
    // claiming that the later five-second abort finished the handler.
    let panic_response = tokio::time::timeout(TEST_LIMIT, panic_request)
        .await
        .unwrap()
        .unwrap();
    assert!(panic_response.is_empty());
    stream_survives_initial_grace(&mut stream).await;
    stream_closed(stream).await;
    assert!(usage_request.await.unwrap().is_empty());
    assert!(!retirement.drained());
    assert!(!task.is_finished());
    assert_eq!(store.head().unwrap(), before);

    release_usage.send(()).unwrap();
    let outcome = tokio::time::timeout(TEST_LIMIT, task)
        .await
        .unwrap()
        .unwrap();
    assert!(retirement.drained());
    assert_ne!(store.head().unwrap(), before);
    assert_reopened_commit(&directory, &store);
    eventually(|| Arc::strong_count(&retirement) == 1).await;
    assert!(
        outcome.is_err(),
        "lifecycle B1: an internal connection panic during graceful drain must not become Ok"
    );
}
