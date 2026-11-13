//! In-memory CONNECT acceptance/refusal contracts; no socket is opened.
use super::frontend::{ConnectState, FrontendError, parse_connect_head};

const HEAD: &[u8] = b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\n";

#[test]
fn egress_frontend_borrows_the_exact_authority_without_authorizing_a_tunnel() {
    let parsed = parse_connect_head(HEAD).unwrap().unwrap();
    assert_eq!(parsed.raw_authority(), "api.example.test:443");
    let source = HEAD.as_ptr() as usize;
    let borrowed = parsed.raw_authority().as_ptr() as usize;
    assert!((source..source + HEAD.len()).contains(&borrowed));
    let mut state = ConnectState::new(100);
    assert_eq!(
        state
            .receive_head(HEAD, 100)
            .unwrap()
            .unwrap()
            .raw_authority(),
        "api.example.test:443"
    );
    assert!(!state.is_tunnel());
    assert_eq!(state.tunnel_data(b"early", 101), Err(FrontendError::State));
    assert!(state.is_closed());
}

#[test]
fn egress_frontend_success_waits_for_a_current_broker_acceptance() {
    let mut state = ConnectState::new(0);
    assert!(state.receive_head(&HEAD[..20], 1).unwrap().is_none());
    assert!(state.receive_head(HEAD, 2).unwrap().is_some());
    assert_eq!(
        state.broker_accepted(3).unwrap(),
        b"HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    assert!(state.is_tunnel());
    assert_eq!(state.tunnel_data(b"\0opaque\xff", 4), Ok(()));
    assert_eq!(state.broker_accepted(5), Err(FrontendError::State));
    assert!(state.is_closed());
}

#[test]
fn egress_frontend_rejects_http_ambiguity_and_unsupported_methods() {
    let fixtures: &[&[u8]] = &[
        b"GET https://api.example.test/ HTTP/1.1\r\nHost: api.example.test\r\n\r\n",
        b"CONNECT-UDP api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.0\r\nHost: api.example.test:443\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: other.example.test:443\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\nhost: api.example.test:443\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\nContent-Length: 0\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\nAuthorization: synthetic\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\nProxy-Authorization: synthetic\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\nHost: api.example.test:443\n\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n folded\r\n\r\n",
        b"CONNECT  api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\n",
        b"CONNECT https://api.example.test:443/ HTTP/1.1\r\nHost: https://api.example.test:443/\r\n\r\n",
        b"CONNECT user@api.example.test:443 HTTP/1.1\r\nHost: user@api.example.test:443\r\n\r\n",
        b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\nearly",
    ];
    for input in fixtures {
        assert!(
            parse_connect_head(input).is_err(),
            "accepted fixture index {}",
            fixtures.iter().position(|v| v == input).unwrap()
        );
    }
}

#[test]
fn egress_frontend_enforces_header_and_authority_bounds() {
    assert!(parse_connect_head(&vec![b'x'; 16_385]).is_err());
    let authority = format!("{}:443", "a".repeat(509));
    let oversized = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    assert!(parse_connect_head(oversized.as_bytes()).is_err());
    let mut too_many = HEAD[..HEAD.len() - 2].to_vec();
    for _ in 0..32 {
        too_many.extend_from_slice(b"X: a\r\n");
    }
    too_many.extend_from_slice(b"\r\n");
    assert!(parse_connect_head(&too_many).is_err());
}

#[test]
fn egress_frontend_deadlines_and_clock_rollback_close_without_success() {
    let mut header = ConnectState::new(10);
    assert_eq!(
        header.receive_head(HEAD, 5_010).err(),
        Some(FrontendError::Timeout)
    );
    let mut pending = ConnectState::new(0);
    pending.receive_head(HEAD, 1).unwrap();
    assert_eq!(pending.broker_accepted(10_001), Err(FrontendError::Timeout));
    let mut rollback = ConnectState::new(100);
    assert!(rollback.receive_head(HEAD, 99).is_err());
    assert!(rollback.is_closed());
    let mut idle = ConnectState::new(0);
    idle.receive_head(HEAD, 0).unwrap();
    idle.broker_accepted(1).unwrap();
    assert_eq!(
        idle.tunnel_data(b"later", 60_001),
        Err(FrontendError::Timeout)
    );
}

#[test]
fn egress_frontend_denial_and_channel_loss_are_terminal_for_the_local_flow() {
    let mut state = ConnectState::new(0);
    state.receive_head(HEAD, 1).unwrap();
    assert_eq!(
        state.broker_rejected(2).unwrap(),
        b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    assert!(state.is_closed());
    let mut accepted = ConnectState::new(0);
    accepted.receive_head(HEAD, 0).unwrap();
    accepted.broker_accepted(1).unwrap();
    accepted.close();
    assert_eq!(accepted.tunnel_data(b"late", 2), Err(FrontendError::State));
}
