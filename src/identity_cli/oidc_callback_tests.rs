//! Real loopback callback regressions with synthetic values only. NOT_RUN.
use super::*;
use std::net::SocketAddr;
use tokio::{task::JoinHandle, time::timeout};

const ISSUER: &str = "https://issuer.example.test/callback-fixture";
const STATE: &str = "public-callback-state-canary";
const CODE: &str = "public-callback-code-canary";
const TEST_BOUND: Duration = Duration::from_secs(5);

struct Receiver {
    address: SocketAddr,
    authority: String,
    task: JoinHandle<Result<Callback>>,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Receiver {
    async fn start(deadline: Instant) -> Result<Self> {
        let listener = CallbackListener::bind().await?;
        let address = listener.listener.local_addr()?;
        let authority = listener.authority.clone();
        let task = tokio::spawn(listener.receive(ISSUER, STATE, deadline));
        Ok(Self {
            address,
            authority,
            task,
        })
    }
    async fn exchange(&self, target: &str) -> Result<Vec<u8>> {
        let mut stream = timeout(TEST_BOUND, TcpStream::connect(self.address)).await??;
        stream
            .write_all(
                format!("GET {target} HTTP/1.1\r\nHost: {}\r\n\r\n", self.authority).as_bytes(),
            )
            .await?;
        let mut response = Vec::new();
        timeout(TEST_BOUND, stream.take(4096).read_to_end(&mut response)).await??;
        Ok(response)
    }
    async fn finish(&mut self) -> Result<Callback> {
        timeout(TEST_BOUND, &mut self.task).await??
    }
    async fn assert_closed(&self) -> Result<()> {
        assert!(
            timeout(TEST_BOUND, TcpStream::connect(self.address))
                .await?
                .is_err()
        );
        Ok(())
    }
}
fn good_target() -> String {
    format!("{PATH}?state={STATE}&code={CODE}")
}
fn fixed_response(response: &[u8], status: &str) {
    assert!(response.starts_with(format!("HTTP/1.1 {status}\r\n").as_bytes()));
    let text = String::from_utf8_lossy(response);
    for private in [
        STATE,
        CODE,
        "public-wrong-state-canary",
        "public-error-canary",
    ] {
        assert!(
            !text.contains(private),
            "callback parameters must not be reflected"
        );
    }
}
fn exact_code(callback: Callback) {
    assert!(matches!(callback, Callback::Code { code, state } if code == CODE && state == STATE));
}

#[tokio::test]
async fn invalid_peer_then_exact_callback_succeeds() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    fixed_response(&receiver.exchange("/unrelated").await?, "400 Bad Request");
    assert!(!receiver.task.is_finished());
    fixed_response(&receiver.exchange(&good_target()).await?, "200 OK");
    exact_code(receiver.finish().await?);
    receiver.assert_closed().await
}

#[tokio::test]
async fn wrong_state_code_and_refusal_do_not_consume_the_pending_attempt() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    for outcome in [
        format!("code={CODE}"),
        "error=access_denied&error_description=public-error-canary".into(),
    ] {
        fixed_response(
            &receiver
                .exchange(&format!("{PATH}?state=public-wrong-state-canary&{outcome}"))
                .await?,
            "400 Bad Request",
        );
        assert!(!receiver.task.is_finished());
    }
    fixed_response(&receiver.exchange(&good_target()).await?, "200 OK");
    exact_code(receiver.finish().await?);
    Ok(())
}

#[tokio::test]
async fn duplicate_parameters_and_wrong_issuer_remain_strict_before_continuation() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    for target in [
        format!("{PATH}?state={STATE}&state={STATE}&code={CODE}"),
        format!("{}&iss=https%3A%2F%2Fother.example.test", good_target()),
    ] {
        fixed_response(&receiver.exchange(&target).await?, "400 Bad Request");
        assert!(!receiver.task.is_finished());
    }
    fixed_response(&receiver.exchange(&good_target()).await?, "200 OK");
    exact_code(receiver.finish().await?);
    Ok(())
}

#[tokio::test]
async fn correlated_refusal_is_terminal_and_closes_the_listener() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    fixed_response(
        &receiver
            .exchange(&format!(
                "{PATH}?state={STATE}&error=access_denied&error_description=public-error-canary"
            ))
            .await?,
        "400 Bad Request",
    );
    assert!(matches!(receiver.finish().await?, Callback::Refused { state } if state == STATE));
    receiver.assert_closed().await
}

#[tokio::test]
async fn exact_callback_as_eighth_connection_still_succeeds() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    for _ in 0..7 {
        fixed_response(&receiver.exchange("/unrelated").await?, "400 Bad Request");
    }
    fixed_response(&receiver.exchange(&good_target()).await?, "200 OK");
    exact_code(receiver.finish().await?);
    receiver.assert_closed().await
}

#[tokio::test]
async fn eight_invalid_connections_exhaust_the_original_attempt() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    for _ in 0..8 {
        fixed_response(&receiver.exchange("/unrelated").await?, "400 Bad Request");
    }
    let result = receiver.finish().await;
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("connection limit")
    );
    receiver.assert_closed().await
}

#[tokio::test]
async fn invalid_peer_and_slow_peer_do_not_rearm_the_initial_deadline() -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut receiver = Receiver::start(deadline).await?;
    fixed_response(&receiver.exchange("/unrelated").await?, "400 Bad Request");
    let mut slow = TcpStream::connect(receiver.address).await?;
    slow.write_all(b"GET /oidc/callback?").await?;
    let result = timeout_at(deadline + Duration::from_secs(1), &mut receiver.task).await??;
    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("deadline"));
    receiver.assert_closed().await
}

#[tokio::test]
async fn already_expired_deadline_refuses_a_queued_exact_callback() -> Result<()> {
    let listener = CallbackListener::bind().await?;
    let address = listener.listener.local_addr()?;
    let mut peer = TcpStream::connect(address).await?;
    peer.write_all(
        format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\n\r\n",
            good_target(),
            listener.authority
        )
        .as_bytes(),
    )
    .await?;
    assert!(
        listener
            .receive(ISSUER, STATE, Instant::now())
            .await
            .is_err()
    );
    assert!(TcpStream::connect(address).await.is_err());
    Ok(())
}

#[tokio::test]
async fn slow_first_peer_is_bounded_then_exact_callback_succeeds() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + Duration::from_secs(8)).await?;
    let mut slow = TcpStream::connect(receiver.address).await?;
    slow.write_all(b"GET /oidc/callback?").await?;
    let started = Instant::now();
    let mut response = Vec::new();
    timeout(
        PARSE_TIMEOUT + Duration::from_secs(1),
        slow.read_to_end(&mut response),
    )
    .await??;
    fixed_response(&response, "400 Bad Request");
    assert!(started.elapsed() < PARSE_TIMEOUT + Duration::from_secs(1));
    fixed_response(&receiver.exchange(&good_target()).await?, "200 OK");
    exact_code(receiver.finish().await?);
    Ok(())
}

#[tokio::test]
async fn cancellation_after_invalid_peer_closes_listener_and_pending_connection() -> Result<()> {
    let mut receiver = Receiver::start(Instant::now() + TEST_BOUND).await?;
    fixed_response(&receiver.exchange("/unrelated").await?, "400 Bad Request");
    let mut peer = TcpStream::connect(receiver.address).await?;
    peer.write_all(b"GET /oidc/callback?").await?;
    receiver.task.abort();
    assert!(matches!((&mut receiver.task).await, Err(cause) if cause.is_cancelled()));
    receiver.assert_closed().await?;
    let mut byte = [0_u8; 1];
    match timeout(TEST_BOUND, peer.read(&mut byte)).await? {
        Ok(count) => assert_eq!(count, 0),
        Err(cause) => assert!(matches!(
            cause.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        )),
    }
    Ok(())
}
