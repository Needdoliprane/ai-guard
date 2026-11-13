//! Real connection JoinError and an in-branch ordering observer; NOT_RUN here.
use super::*;
use axum::{http::StatusCode, routing::get};
use tokio::io::AsyncWriteExt;

async fn panic_handler() -> StatusCode {
    panic!("synthetic connection owner failure")
}

#[tokio::test]
async fn connection_join_error_retires_humans_before_signalling_acceptors() {
    let (_fixture, state) = crate::api::owned_human_fixture_for_test();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, _) = watch::channel(false);
    let retirement = Arc::new(state.control_recovery_retirement());
    let human = Some(Arc::new(state.human_retirement().unwrap()));
    let (observed, observation) = tokio::sync::oneshot::channel();
    let checked = state.clone();
    let before_stop = Box::new(move || {
        let _ = observed.send(checked.human_admission_closed_for_test());
    });
    let router = Router::new().route("/synthetic-panic", get(panic_handler));
    let owner = http_owner_inner(
        listener,
        router,
        stop.clone(),
        retirement,
        human,
        Some(before_stop),
    );
    let serving = tokio::spawn(owner);
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    connection
        .write_all(b"GET /synthetic-panic HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), observation)
            .await
            .unwrap()
            .unwrap(),
        "human admission must already refuse inside the JoinError branch before stop publication"
    );
    let result = tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    assert!(*stop.borrow());
}
