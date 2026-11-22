//! `egress_broker_*`: the real worker helper and this owner over an owned
//! in-memory pipe, an owned loopback upstream fixture that counts its accepts,
//! and a scripted gateway port. No DNS, engine or external network is used.

use super::*;
use crate::broker::egress_owner::resolve::{HostLookup, InternalService, LookupFuture};
use crate::egress::{
    EgressTransport,
    channel_helper::{self, LoopbackConfig},
    ids::DigestV1,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll};
use tokio::{io::ReadBuf, net::TcpListener};

const FIXTURE: &str = "fixture.egress.test";

#[derive(Default)]
struct Gateway {
    allow: AtomicBool,
    decide_unavailable: AtomicBool,
    observe_unavailable: AtomicBool,
    decision_delay: Duration,
    remaining_ms: Option<u64>,
    decisions: Mutex<Vec<EgressDecideRequestV1>>,
    observations: Mutex<Vec<EgressObservationV1>>,
    revoke: Mutex<Vec<IdV1>>,
}
impl EgressControl for Gateway {
    fn decide(&self, request: EgressDecideRequestV1) -> ControlFuture<'_, EgressDecisionV1> {
        Box::pin(async move {
            if self.decide_unavailable.load(Ordering::SeqCst) {
                return Err(ControlError::Unavailable);
            }
            if !self.decision_delay.is_zero() {
                tokio::time::sleep(self.decision_delay).await;
            }
            let allow = self.allow.load(Ordering::SeqCst);
            self.decisions.lock().unwrap().push(request.clone());
            Ok(EgressDecisionV1 {
                version: 1,
                decision_id: request.decision_id,
                outcome: if allow {
                    EgressOutcomeV1::Allow
                } else {
                    EgressOutcomeV1::Deny
                },
                reason: if allow {
                    EgressDecisionReasonV1::AllowedByPolicy
                } else {
                    EgressDecisionReasonV1::NotInPolicy
                },
                policy: Some(EgressPolicyRefV1 {
                    source: EgressPolicySourceV1::OperatorFile,
                    revision: 1,
                    digest: DigestV1::of(b"fixture policy"),
                }),
                grant_id: None,
                remaining_ms: allow.then_some(self.remaining_ms.unwrap_or(60_000)),
                receipt: EgressReceiptV1 {
                    audit_seq: 7,
                    audit_hash: "a".repeat(64),
                },
            })
        })
    }
    fn observe(&self, request: EgressObserveRequestV1) -> ControlFuture<'_, ()> {
        Box::pin(async move {
            if self.observe_unavailable.load(Ordering::SeqCst) {
                return Err(ControlError::Unavailable);
            }
            self.observations.lock().unwrap().push(request.observation);
            Ok(())
        })
    }
    fn renew(&self, request: EgressRenewRequestV1) -> ControlFuture<'_, EgressRenewResponseV1> {
        Box::pin(async move {
            let revoke = self.revoke.lock().unwrap().clone();
            Ok(EgressRenewResponseV1 {
                version: 1,
                revoked: request
                    .decisions
                    .into_iter()
                    .filter(|id| revoke.contains(id))
                    .collect(),
                expired: vec![],
            })
        })
    }
}

struct NoLookup(AtomicUsize);
impl HostLookup for NoLookup {
    fn lookup(&self, _: String, _: u16) -> LookupFuture {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(std::io::Error::other("no DNS in this test")) })
    }
}

struct PendingLookup(AtomicUsize);
impl HostLookup for PendingLookup {
    fn lookup(&self, _: String, _: u16) -> LookupFuture {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

/// An owned loopback echo service counting accepted upstream sockets.
struct Upstream {
    port: u16,
    accepts: Arc<AtomicUsize>,
    closed: Arc<tokio::sync::Notify>,
}
async fn upstream() -> Upstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepts = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(tokio::sync::Notify::new());
    let (count, notify) = (accepts.clone(), closed.clone());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let notify = notify.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                loop {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            if socket.write_all(&buffer[..count]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                notify.notify_one();
            });
        }
    });
    Upstream {
        port,
        accepts,
        closed,
    }
}

struct Harness {
    gateway: Arc<Gateway>,
    lookups: Arc<NoLookup>,
    helper_port: u16,
    owner: tokio::task::JoinHandle<ChannelExit>,
    stop: watch::Sender<bool>,
}

fn fixture_binding() -> EgressBindingV1 {
    EgressBindingV1 {
        session_id: "00000000-0000-4000-8000-0000000000a1".into(),
        worker_id: "00000000-0000-4000-8000-0000000000a2".into(),
        attempt_id: "00000000-0000-4000-8000-0000000000a3".into(),
        channel_id: IdV1::random(),
        epoch: 1,
        context_digest: DigestV1::of(b"fixture worker A1"),
    }
}

fn fixture_resolver(upstream_port: u16, lookup: Arc<NoLookup>) -> Arc<Resolver> {
    let authority =
        Authority::new(FIXTURE, upstream_port, EgressTransport::ConnectTcpOpaque).unwrap();
    Arc::new(
        Resolver::new(
            lookup,
            vec![InternalService {
                authority,
                addresses: vec!["127.0.0.1".parse().unwrap()],
            }],
            vec![],
        )
        .unwrap(),
    )
}

async fn harness(gateway: Arc<Gateway>, upstream_port: u16) -> Harness {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let helper_port = probe.local_addr().unwrap().port();
    drop(probe);
    let (broker_end, helper_end) = tokio::io::duplex(256 * 1024);
    let (helper_input, helper_output) = tokio::io::split(helper_end);
    tokio::spawn(channel_helper::serve(
        LoopbackConfig::new(helper_port).unwrap(),
        helper_input,
        helper_output,
    ));
    let lookups = Arc::new(NoLookup(AtomicUsize::new(0)));
    let resolver = fixture_resolver(upstream_port, lookups.clone());
    let binding = fixture_binding();
    let times = ChannelTimes {
        lease: Duration::from_millis(1_500),
        renew_every: Duration::from_millis(200),
        open_budget: Duration::from_secs(5),
        connect_attempt: Duration::from_secs(2),
    };
    let (stop, stopped) = watch::channel(false);
    let control: Arc<dyn EgressControl> = gateway.clone();
    let owner = tokio::spawn(run(broker_end, binding, control, resolver, times, stopped));
    Harness {
        gateway,
        lookups,
        helper_port,
        owner,
        stop,
    }
}

/// A worker-side client: CONNECT through the loopback helper.
async fn connect(helper_port: u16, authority: &str) -> (TcpStream, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut client = loop {
        match TcpStream::connect(("127.0.0.1", helper_port)).await {
            Ok(client) => break client,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(error) => panic!("helper never listened: {error}"),
        }
    };
    let head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    client.write_all(head.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        match timeout_at(deadline, client.read(&mut byte))
            .await
            .expect("response deadline")
        {
            Ok(1) => response.push(byte[0]),
            _ => break,
        }
    }
    (client, String::from_utf8_lossy(&response).into_owned())
}

fn gateway(allow: bool) -> Arc<Gateway> {
    let gateway = Gateway::default();
    gateway.allow.store(allow, Ordering::SeqCst);
    Arc::new(gateway)
}

fn fixture_open_request() -> (EgressDecideRequestV1, Authority) {
    let authority = Authority::new(FIXTURE, 443, EgressTransport::ConnectTcpOpaque).unwrap();
    let request = EgressDecideRequestV1 {
        version: 1,
        decision_id: IdV1::random(),
        binding: fixture_binding(),
        flow_id: 1,
        authority: format!("{FIXTURE}:443"),
    };
    (request, authority)
}

#[tokio::test]
async fn egress_broker_late_allow_receipt_expires_before_resolution() {
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        decision_delay: Duration::from_millis(30),
        remaining_ms: Some(1),
        ..Gateway::default()
    });
    let lookup = Arc::new(NoLookup(AtomicUsize::new(0)));
    let resolver = Arc::new(Resolver::new(lookup.clone(), vec![], vec![]).unwrap());
    let (request, authority) = fixture_open_request();

    let result = open_upstream(
        control.clone(),
        resolver,
        ChannelTimes::default(),
        request,
        authority,
    )
    .await;

    assert_eq!(control.decisions.lock().unwrap().len(), 1);
    assert!(matches!(
        result,
        Err(Refusal {
            reason: EgressReasonV1::Expired,
            failure: None
        })
    ));
    assert_eq!(lookup.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn egress_broker_receipt_expiry_stops_pending_resolution() {
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(120),
        ..Gateway::default()
    });
    let lookup = Arc::new(PendingLookup(AtomicUsize::new(0)));
    let resolver = Arc::new(Resolver::new(lookup.clone(), vec![], vec![]).unwrap());
    let (request, authority) = fixture_open_request();

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        open_upstream(
            control,
            resolver,
            ChannelTimes::default(),
            request,
            authority,
        ),
    )
    .await
    .expect("resolution ends at receipt expiry, before the processing budget");

    assert_eq!(lookup.0.load(Ordering::SeqCst), 1);
    assert!(matches!(
        result,
        Err(Refusal {
            reason: EgressReasonV1::Expired,
            failure: Some(("resolution_timeout", "host_resolver"))
        })
    ));
}

/// Stops writes after the selected number of complete frames while preserving
/// full-duplex reads. A notification proves the write was actually attempted.
struct BlockedPublication {
    inner: tokio::io::DuplexStream,
    remaining_frames: usize,
    prefix_after_frames: usize,
    blocked: Arc<tokio::sync::Notify>,
}

impl AsyncRead for BlockedPublication {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for BlockedPublication {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.remaining_frames == 0 && self.prefix_after_frames == 0 {
            self.blocked.notify_one();
            return Poll::Pending;
        }
        let bytes = if self.remaining_frames == 0 {
            &buf[..buf.len().min(self.prefix_after_frames)]
        } else {
            buf
        };
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if self.remaining_frames == 0
            && let Poll::Ready(Ok(written)) = result
        {
            self.prefix_after_frames -= written;
        }
        if result.is_pending() {
            self.blocked.notify_one();
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.remaining_frames -= 1;
        }
        result
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct BlockedHarness {
    helper: tokio::io::DuplexStream,
    owner: tokio::task::JoinHandle<ChannelExit>,
    blocked: Arc<tokio::sync::Notify>,
    _stop: watch::Sender<bool>,
}

async fn open_with_blocked_publication(
    gateway: Arc<Gateway>,
    upstream_port: u16,
    complete_frames: usize,
    prefix_after_frames: usize,
) -> BlockedHarness {
    let resolver = fixture_resolver(upstream_port, Arc::new(NoLookup(AtomicUsize::new(0))));
    let (broker, mut helper) = tokio::io::duplex(4096);
    let blocked = Arc::new(tokio::sync::Notify::new());
    let broker = BlockedPublication {
        inner: broker,
        remaining_frames: complete_frames,
        prefix_after_frames,
        blocked: blocked.clone(),
    };
    let (_stop, stopped) = watch::channel(false);
    let owner = tokio::spawn(run(
        broker,
        fixture_binding(),
        gateway,
        resolver,
        ChannelTimes::default(),
        stopped,
    ));
    let handshake_deadline = Instant::now() + HANDSHAKE_TIME;
    let hello = channel_protocol::read_handshake(&mut helper, handshake_deadline)
        .await
        .unwrap();
    channel_protocol::write_handshake(&mut helper, &hello, handshake_deadline)
        .await
        .unwrap();
    channel_protocol::write_message(
        &mut helper,
        &EgressMessageV1::Open {
            flow_id: 1,
            authority: format!("{FIXTURE}:{upstream_port}"),
        },
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    BlockedHarness {
        helper,
        owner,
        blocked,
        _stop,
    }
}

async fn expiry_during_publication(complete_frames: usize) {
    let upstream = upstream().await;
    let gateway = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(300),
        ..Gateway::default()
    });
    let BlockedHarness {
        mut helper,
        owner,
        blocked,
        _stop,
    } = open_with_blocked_publication(gateway.clone(), upstream.port, complete_frames, 0).await;

    let deadline = Instant::now() + Duration::from_secs(2);
    timeout_at(deadline, blocked.notified()).await.unwrap();
    timeout_at(deadline, upstream.closed.notified())
        .await
        .expect("the upstream socket closes at receipt expiry, before the frame timeout");
    assert_eq!(
        timeout_at(deadline, owner).await.unwrap().unwrap(),
        ChannelExit::Protocol
    );
    let mut frames = EgressFrameReader::new();
    if complete_frames == 2 {
        assert!(matches!(
            frames.read(&mut helper).await.unwrap(),
            Some(EgressMessageV1::Accepted { flow_id: 1 })
        ));
    }
    assert!(frames.read(&mut helper).await.unwrap().is_none());
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 1);
    assert_expired_connection(&gateway);
}

fn assert_expired_connection(gateway: &Gateway) {
    let observations = gateway.observations.lock().unwrap();
    assert!(observations.iter().any(|event| matches!(
        event,
        EgressObservationV1::Connection {
            flow_id: 1,
            connected: true,
            ..
        }
    )));
    assert!(observations.iter().any(|event| matches!(
        event,
        EgressObservationV1::Closed {
            flow_id: 1,
            reason: EgressCloseReasonV1::Expired,
            bytes_up: 0,
            bytes_down: 0,
            ..
        }
    )));
}

#[tokio::test]
async fn egress_broker_receipt_expiry_closes_socket_during_accept_publication() {
    expiry_during_publication(1).await;
}

#[tokio::test]
async fn egress_broker_receipt_expiry_closes_socket_during_credit_publication() {
    expiry_during_publication(2).await;
}

#[tokio::test]
async fn egress_broker_revocation_closes_before_a_blocked_reset_frame() {
    let upstream = upstream().await;
    let gateway = gateway(true);
    let BlockedHarness {
        mut helper,
        owner,
        blocked,
        _stop,
    } = open_with_blocked_publication(gateway.clone(), upstream.port, 3, 0).await;
    let mut frames = EgressFrameReader::new();
    assert!(matches!(
        frames.read(&mut helper).await.unwrap(),
        Some(EgressMessageV1::Accepted { flow_id: 1 })
    ));
    assert!(matches!(
        frames.read(&mut helper).await.unwrap(),
        Some(EgressMessageV1::Credit { flow_id: 1, .. })
    ));
    let decision_id = gateway.decisions.lock().unwrap()[0].decision_id.clone();
    gateway.revoke.lock().unwrap().push(decision_id);

    let deadline = Instant::now() + Duration::from_secs(4);
    timeout_at(deadline, blocked.notified())
        .await
        .expect("the owner attempts the reset");
    timeout_at(deadline, upstream.closed.notified())
        .await
        .expect("the socket closes despite the blocked reset frame");
    timeout_at(deadline, async {
        while !gateway.observations.lock().unwrap().iter().any(|event| {
            matches!(
                event,
                EgressObservationV1::Closed {
                    flow_id: 1,
                    reason: EgressCloseReasonV1::Revoked,
                    bytes_up: 0,
                    bytes_down: 0,
                    ..
                }
            )
        }) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the actual closure is observed independently of the pipe");
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 1);
    assert!(!owner.is_finished());
    owner.abort();
    let _ = owner.await;
}

#[tokio::test]
async fn egress_broker_loss_of_authority_interrupts_a_partially_written_command() {
    for reason in [
        EgressCloseReasonV1::Revoked,
        EgressCloseReasonV1::OwnerExit,
        EgressCloseReasonV1::Expired,
        EgressCloseReasonV1::LeaseLost,
    ] {
        let (io, mut peer) = tokio::io::duplex(3);
        let blocked = Arc::new(tokio::sync::Notify::new());
        let io = BlockedPublication {
            inner: io,
            remaining_frames: usize::MAX,
            prefix_after_frames: 0,
            blocked: blocked.clone(),
        };
        let (events, _incoming) = mpsc::channel(1);
        let started = Instant::now();
        let short = started + Duration::from_millis(300);
        let long = started + Duration::from_secs(60);
        let valid_until = if reason == EgressCloseReasonV1::Expired {
            short
        } else {
            long
        };
        let lease_until = if reason == EgressCloseReasonV1::LeaseLost {
            short
        } else {
            long
        };
        let socket = SocketControl::spawn(1, io, events, valid_until);
        assert!(socket.arm_lease(lease_until));
        assert!(
            socket
                .commands
                .try_send(Command::Write(b"accepted prefix".to_vec()))
                .is_ok()
        );

        let deadline = started + Duration::from_secs(2);
        timeout_at(deadline, blocked.notified())
            .await
            .expect("the commanded write is pending");
        let (closed, expected_at) = match reason {
            EgressCloseReasonV1::Expired | EgressCloseReasonV1::LeaseLost => (
                timeout_at(deadline, socket.closed).await.unwrap().unwrap(),
                short,
            ),
            _ => {
                let signalled = Instant::now();
                let closing = socket.stop(1, IdV1::random(), started, reason);
                (
                    timeout_at(deadline, closing.closed).await.unwrap().unwrap(),
                    signalled,
                )
            }
        };
        let mut received = Vec::new();
        timeout_at(deadline, peer.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, b"acc");
        assert_eq!(closed.reason, reason);
        assert_eq!(closed.traffic.bytes_up, 3);
        assert_eq!(closed.traffic.bytes_down, 0);
        assert!(closed.at >= expected_at);
    }
}

fn fixture_owner<W>(
    output: W,
    gateway: Arc<Gateway>,
    upstream_port: u16,
    times: ChannelTimes,
) -> (Owner<W>, mpsc::Receiver<Event>) {
    let mut ledger = EgressChannelState::new(0);
    ledger
        .accept_capability(Some(&EgressCapabilityV1::default()), 0)
        .unwrap();
    let (events, incoming) = mpsc::channel(8);
    let owner = Owner {
        output: Some(output),
        binding: fixture_binding(),
        control: gateway,
        resolver: fixture_resolver(upstream_port, Arc::new(NoLookup(AtomicUsize::new(0)))),
        times,
        ledger,
        started: Instant::now(),
        flows: HashMap::new(),
        events,
        observations: JoinSet::new(),
        observation_slots: Arc::new(Semaphore::new(OBSERVATION_SLOTS)),
        observations_lost: 0,
        renewing: false,
    };
    (owner, incoming)
}

#[tokio::test]
async fn egress_broker_receipt_expiry_closes_socket_before_event_consumption() {
    let upstream = upstream().await;
    let gateway = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(300),
        ..Gateway::default()
    });
    let (mut owner, mut incoming) = fixture_owner(
        tokio::io::sink(),
        gateway.clone(),
        upstream.port,
        ChannelTimes::default(),
    );
    owner
        .worker_frame(EgressMessageV1::Open {
            flow_id: 1,
            authority: format!("{FIXTURE}:{}", upstream.port),
        })
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    timeout_at(deadline, upstream.closed.notified())
        .await
        .expect("the socket closes even while its event is queued");
    let event = timeout_at(deadline, incoming.recv())
        .await
        .unwrap()
        .unwrap();
    owner.event(event).await.unwrap();
    assert!(owner.flows.is_empty());
    owner.shutdown(ChannelExit::Stopped).await;
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 1);
    assert_expired_connection(&gateway);
}

#[tokio::test]
async fn egress_broker_discards_buffered_data_after_receipt_or_lease_expiry() {
    for reason in [EgressCloseReasonV1::Expired, EgressCloseReasonV1::LeaseLost] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut times = ChannelTimes::default();
        if reason == EgressCloseReasonV1::LeaseLost {
            times.lease = Duration::from_millis(300);
        }
        let control = Arc::new(Gateway {
            allow: AtomicBool::new(true),
            remaining_ms: (reason == EgressCloseReasonV1::Expired).then_some(300),
            ..Gateway::default()
        });
        let (output, mut helper) = tokio::io::duplex(4096);
        let (mut owner, mut incoming) = fixture_owner(output, control.clone(), port, times);
        let deadline = Instant::now() + Duration::from_secs(2);
        owner
            .worker_frame(EgressMessageV1::Open {
                flow_id: 1,
                authority: format!("{FIXTURE}:{port}"),
            })
            .await
            .unwrap();
        let (mut upstream, _) = timeout_at(deadline, listener.accept())
            .await
            .unwrap()
            .unwrap();
        let connected = timeout_at(deadline, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        owner.event(connected).await.unwrap();
        let mut frames = EgressFrameReader::new();
        assert!(matches!(
            frames.read(&mut helper).await.unwrap(),
            Some(EgressMessageV1::Accepted { flow_id: 1 })
        ));
        assert!(matches!(
            frames.read(&mut helper).await.unwrap(),
            Some(EgressMessageV1::Credit { flow_id: 1, .. })
        ));
        owner
            .worker_frame(EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 32,
            })
            .await
            .unwrap();
        upstream.write_all(b"old").await.unwrap();
        let data = timeout_at(deadline, incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&data, Event::Upstream { flow_id: 1, data } if data == b"old"));

        assert_eq!(
            timeout_at(deadline, upstream.read(&mut [0; 1]))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        owner.event(data).await.unwrap();
        drop(owner.output.take());
        assert!(
            frames.read(&mut helper).await.unwrap().is_none(),
            "expired data must not be published"
        );
        owner.shutdown(ChannelExit::Stopped).await;
        assert!(control.observations.lock().unwrap().iter().any(|event| matches!(event,
            EgressObservationV1::Closed { flow_id: 1, reason: observed, bytes_down: 3, .. } if *observed == reason)));
    }
}

#[tokio::test]
async fn egress_broker_expiry_during_partial_data_frame_closes_the_helper_channel() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(300),
        ..Gateway::default()
    });
    let BlockedHarness {
        mut helper,
        owner,
        blocked,
        _stop,
    } = open_with_blocked_publication(control.clone(), port, 3, 2).await;
    let deadline = Instant::now() + Duration::from_secs(2);
    let (mut upstream, _) = timeout_at(deadline, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut frames = EgressFrameReader::new();
    assert!(matches!(
        frames.read(&mut helper).await.unwrap(),
        Some(EgressMessageV1::Accepted { flow_id: 1 })
    ));
    assert!(matches!(
        frames.read(&mut helper).await.unwrap(),
        Some(EgressMessageV1::Credit { flow_id: 1, .. })
    ));
    channel_protocol::write_message(
        &mut helper,
        &EgressMessageV1::Credit {
            flow_id: 1,
            bytes: 32,
        },
        deadline,
    )
    .await
    .unwrap();
    upstream.write_all(b"data").await.unwrap();
    timeout_at(deadline, blocked.notified())
        .await
        .expect("the writer is pending after emitting a partial data frame");
    assert_eq!(
        timeout_at(deadline, owner)
            .await
            .expect("the data deadline closes the helper channel")
            .unwrap(),
        ChannelExit::Protocol,
    );
    assert!(matches!(
        frames.read(&mut helper).await,
        Err(channel_protocol::CommonEgressChannelError::PartialFrame)
    ));
    assert!(
        control
            .observations
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                EgressObservationV1::Closed {
                    flow_id: 1,
                    reason: EgressCloseReasonV1::Expired,
                    bytes_down: 4,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn egress_broker_allowed_destination_is_decided_durably_before_one_pinned_connect() {
    let upstream = upstream().await;
    let harness = harness(gateway(true), upstream.port).await;
    let authority = format!("{FIXTURE}:{}", upstream.port);
    let (mut client, response) = connect(harness.helper_port, &authority).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    client.write_all(b"ping through the broker").await.unwrap();
    let mut echoed = [0u8; 23];
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"ping through the broker");
    assert_eq!(
        upstream.accepts.load(Ordering::SeqCst),
        1,
        "exactly one upstream socket"
    );
    let decisions = harness.gateway.decisions.lock().unwrap().clone();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].authority, authority);
    assert_eq!(
        harness.lookups.0.load(Ordering::SeqCst),
        0,
        "internal service: no DNS"
    );
    let observations = harness.gateway.observations.lock().unwrap().clone();
    assert!(matches!(
        observations.first(),
        Some(EgressObservationV1::ChannelOpened { .. })
    ));
    assert!(observations.iter().any(|o| matches!(o,
        EgressObservationV1::Connection { connected: true, peer: Some(peer), resolver, .. }
            if *peer == format!("127.0.0.1:{}", upstream.port) && resolver == "internal_service")));
    harness.stop.send(true).unwrap();
    assert_eq!(harness.owner.await.unwrap(), ChannelExit::Stopped);
}

#[tokio::test]
async fn egress_broker_unknown_destination_is_refused_without_resolution_or_connect() {
    let upstream = upstream().await;
    let harness = harness(gateway(false), upstream.port).await;
    let (_client, response) =
        connect(harness.helper_port, &format!("{FIXTURE}:{}", upstream.port)).await;
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    let (_client, response) = connect(harness.helper_port, "unknown.example.test:443").await;
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 0);
    assert_eq!(
        harness.lookups.0.load(Ordering::SeqCst),
        0,
        "a denied name is never resolved"
    );
    assert_eq!(
        harness.gateway.decisions.lock().unwrap().len(),
        2,
        "each refusal was decided centrally"
    );
    harness.stop.send(true).unwrap();
}

#[tokio::test]
async fn egress_broker_audit_outage_blocks_the_connection() {
    let upstream = upstream().await;
    let unavailable = gateway(true);
    unavailable.decide_unavailable.store(true, Ordering::SeqCst);
    let harness = harness(unavailable, upstream.port).await;
    let (_client, response) =
        connect(harness.helper_port, &format!("{FIXTURE}:{}", upstream.port)).await;
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert_eq!(
        upstream.accepts.load(Ordering::SeqCst),
        0,
        "no decision, no socket"
    );
    harness.stop.send(true).unwrap();
}

#[tokio::test]
async fn egress_broker_refuses_every_flow_when_the_channel_cannot_be_recorded() {
    let upstream = upstream().await;
    let unrecorded = gateway(true);
    unrecorded.observe_unavailable.store(true, Ordering::SeqCst);
    let harness = harness(unrecorded, upstream.port).await;
    assert_eq!(harness.owner.await.unwrap(), ChannelExit::AuditUnavailable);
    assert!(harness.gateway.decisions.lock().unwrap().is_empty());
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn egress_broker_revocation_closes_the_live_upstream_socket_within_the_renewal() {
    let upstream = upstream().await;
    let harness = harness(gateway(true), upstream.port).await;
    let (mut client, response) =
        connect(harness.helper_port, &format!("{FIXTURE}:{}", upstream.port)).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let decision = harness.gateway.decisions.lock().unwrap()[0]
        .decision_id
        .clone();
    harness
        .gateway
        .revoke
        .lock()
        .unwrap()
        .push(decision.clone());
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), upstream.closed.notified())
        .await
        .expect("the broker closed the revoked upstream socket");
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut rest)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let closed = harness.gateway.observations.lock().unwrap().iter().any(|o| matches!(o,
        EgressObservationV1::Closed { reason: EgressCloseReasonV1::Revoked, decision_id, .. } if *decision_id == decision));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut recorded = closed;
    while !recorded && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        recorded = harness
            .gateway
            .observations
            .lock()
            .unwrap()
            .iter()
            .any(|o| {
                matches!(
                    o,
                    EgressObservationV1::Closed {
                        reason: EgressCloseReasonV1::Revoked,
                        ..
                    }
                )
            });
    }
    assert!(
        recorded,
        "the closure is observed with its revocation cause"
    );
    harness.stop.send(true).unwrap();
}
