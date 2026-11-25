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

async fn run<IO>(
    io: IO,
    binding: EgressBindingV1,
    control: Arc<dyn EgressControl>,
    resolver: Arc<Resolver>,
    times: ChannelTimes,
    stop: watch::Receiver<ChannelStopState>,
) -> ChannelTermination
where
    IO: AsyncRead + AsyncWrite + Send + 'static,
{
    let prepared = match prepare(io, binding, stop).await {
        Ok(prepared) => prepared,
        Err(terminated) => return terminated,
    };
    match prepared.open(control.as_ref()).await {
        Ok(opened) => run_opened(opened, control, resolver, times).await,
        Err(terminated) => terminated,
    }
}

#[derive(Default)]
struct Gateway {
    allow: AtomicBool,
    decide_unavailable: AtomicBool,
    observe_unavailable: AtomicBool,
    opening_reply: OpeningReply,
    stop_on_opening_ack: Option<ChannelStop>,
    opening_calls: AtomicUsize,
    decision_delay: Duration,
    decision_hold: Option<Arc<PendingOperation>>,
    connect_permit_reply: ConnectPermitReply,
    connect_permit_gate: Option<Arc<ObservationGate>>,
    prior_observation_reply: Option<ObservationReply>,
    terminal_observation_reply: Option<ObservationReply>,
    remaining_ms: Option<u64>,
    renewal: Option<Arc<ScriptedRenewal>>,
    decisions: Mutex<Vec<EgressDecideRequestV1>>,
    connect_permits: Mutex<Vec<EgressConnectPermitRequestV1>>,
    observations: Mutex<Vec<EgressObservationV1>>,
    revoke: Mutex<Vec<IdV1>>,
}

#[derive(Clone, Copy, Default)]
enum OpeningReply {
    #[default]
    Fresh,
    Replayed,
    DifferentBinding,
}

#[derive(Clone, Copy, Default)]
enum ConnectPermitReply {
    #[default]
    Fresh,
    Replayed,
    DifferentIp,
    Expired,
    Lost,
}

enum ObservationReply {
    Wait(Arc<ObservationGate>),
    WaitAfterCommit(Arc<PendingOperation>),
    LostAfterCommit,
}

struct ObservationGate {
    started: Semaphore,
    release: Semaphore,
}

#[derive(Default)]
struct ScriptedRenewal {
    requested: tokio::sync::Notify,
    release: tokio::sync::Notify,
    answered: tokio::sync::Notify,
}

#[derive(Default)]
struct PendingOperation {
    started: tokio::sync::Notify,
    dropped: tokio::sync::Notify,
}

impl PendingOperation {
    async fn hold(&self) {
        let _drop = NotifyOnDrop(&self.dropped);
        self.started.notify_one();
        std::future::pending::<()>().await;
    }
}

struct NotifyOnDrop<'a>(&'a tokio::sync::Notify);
impl Drop for NotifyOnDrop<'_> {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

impl EgressControl for Gateway {
    fn decide(&self, request: EgressDecideRequestV1) -> ControlFuture<'_, EgressDecisionV1> {
        Box::pin(async move {
            if self.decide_unavailable.load(Ordering::SeqCst) {
                return Err(ControlError::Unavailable);
            }
            if let Some(hold) = &self.decision_hold {
                hold.hold().await;
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
    fn open_channel(
        &self,
        request: EgressChannelOpeningRequestV1,
    ) -> ControlFuture<'_, EgressChannelOpeningReceiptV1> {
        Box::pin(async move {
            self.opening_calls.fetch_add(1, Ordering::SeqCst);
            let disposition = if matches!(self.opening_reply, OpeningReply::Replayed) {
                EgressChannelOpeningDispositionV1::Replayed
            } else {
                self.observe(EgressObserveRequestV1 {
                    version: request.version,
                    observation_id: request.observation_id.clone(),
                    observation: EgressObservationV1::ChannelOpened {
                        binding: request.binding.clone(),
                    },
                })
                .await?;
                EgressChannelOpeningDispositionV1::Fresh
            };
            if let Some(stop) = &self.stop_on_opening_ack {
                stop.stop();
            }
            let mut binding = request.binding;
            if matches!(self.opening_reply, OpeningReply::DifferentBinding) {
                binding.context_digest = DigestV1::of(b"different fixture");
            }
            Ok(EgressChannelOpeningReceiptV1 {
                version: 1,
                observation_id: request.observation_id,
                binding,
                disposition,
                receipt: EgressReceiptV1 {
                    audit_seq: 7,
                    audit_hash: "a".repeat(64),
                },
            })
        })
    }
    fn issue_connect_permit(
        &self,
        request: EgressConnectPermitRequestV1,
    ) -> ControlFuture<'_, EgressConnectPermitReceiptV1> {
        Box::pin(async move {
            let port = self
                .decisions
                .lock()
                .unwrap()
                .iter()
                .find(|decision| decision.decision_id == request.decision_id)
                .and_then(|decision| Authority::parse_connect(&decision.authority).ok())
                .ok_or(ControlError::Refused)?
                .port();
            self.connect_permits.lock().unwrap().push(request.clone());
            if let Some(gate) = &self.connect_permit_gate {
                gate.started.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
            if matches!(self.connect_permit_reply, ConnectPermitReply::Lost) {
                return Err(ControlError::Unavailable);
            }
            let permit_id = request.permit_id().unwrap();
            let mut request = request;
            if matches!(self.connect_permit_reply, ConnectPermitReply::DifferentIp) {
                request.ip = "127.0.0.2".parse().unwrap();
            }
            Ok(EgressConnectPermitReceiptV1 {
                version: 1,
                permit_id,
                request,
                port,
                valid_for_ms: if matches!(self.connect_permit_reply, ConnectPermitReply::Expired) {
                    1
                } else {
                    1_000
                },
                disposition: if matches!(self.connect_permit_reply, ConnectPermitReply::Replayed) {
                    EgressConnectPermitDispositionV1::Replayed
                } else {
                    EgressConnectPermitDispositionV1::Fresh
                },
                receipt: EgressReceiptV1 {
                    audit_seq: 8,
                    audit_hash: "b".repeat(64),
                },
            })
        })
    }
    fn observe(&self, request: EgressObserveRequestV1) -> ControlFuture<'_, ()> {
        Box::pin(async move {
            if self.observe_unavailable.load(Ordering::SeqCst) {
                return Err(ControlError::Unavailable);
            }
            let reply = if matches!(
                request.observation,
                EgressObservationV1::ChannelClosed { .. }
            ) {
                &self.terminal_observation_reply
            } else {
                &self.prior_observation_reply
            };
            if let Some(ObservationReply::Wait(gate)) = reply {
                gate.started.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
            self.observations.lock().unwrap().push(request.observation);
            if let Some(ObservationReply::WaitAfterCommit(pending)) = reply {
                pending.hold().await;
            }
            if matches!(reply, Some(ObservationReply::LostAfterCommit)) {
                Err(ControlError::Unavailable)
            } else {
                Ok(())
            }
        })
    }
    fn renew(&self, request: EgressRenewRequestV1) -> ControlFuture<'_, EgressRenewResponseV1> {
        Box::pin(async move {
            if let Some(renewal) = &self.renewal {
                renewal.requested.notify_one();
                renewal.release.notified().await;
            }
            let revoke = self.revoke.lock().unwrap().clone();
            let answer = EgressRenewResponseV1 {
                version: 1,
                revoked: request
                    .decisions
                    .into_iter()
                    .filter(|id| revoke.contains(id))
                    .collect(),
                expired: vec![],
            };
            if let Some(renewal) = &self.renewal {
                renewal.answered.notify_one();
            }
            Ok(answer)
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

struct HeldLookup(Arc<PendingOperation>);
impl HostLookup for HeldLookup {
    fn lookup(&self, _: String, _: u16) -> LookupFuture {
        let pending = self.0.clone();
        Box::pin(async move {
            pending.hold().await;
            Err(std::io::Error::other("fixture lookup did not finish"))
        })
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
    owner: tokio::task::JoinHandle<ChannelTermination>,
    stop: ChannelStop,
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

async fn start_channel(
    control: Arc<dyn EgressControl>,
    binding: EgressBindingV1,
    upstream_port: u16,
) -> (
    u16,
    Arc<NoLookup>,
    tokio::task::JoinHandle<ChannelTermination>,
    tokio::task::JoinHandle<Result<(), channel_helper::CommonEgressHelperError>>,
    ChannelStop,
) {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let helper_port = probe.local_addr().unwrap().port();
    drop(probe);
    let (broker_end, helper_end) = tokio::io::duplex(256 * 1024);
    let (helper_input, helper_output) = tokio::io::split(helper_end);
    let helper = tokio::spawn(channel_helper::serve(
        LoopbackConfig::new(helper_port).unwrap(),
        helper_input,
        helper_output,
    ));
    let lookups = Arc::new(NoLookup(AtomicUsize::new(0)));
    let resolver = fixture_resolver(upstream_port, lookups.clone());
    let times = ChannelTimes {
        lease: Duration::from_millis(1_500),
        renew_every: Duration::from_millis(200),
        open_budget: Duration::from_secs(5),
        connect_attempt: Duration::from_secs(2),
    };
    let (stop, stopped) = ChannelStop::new();
    let owner = tokio::spawn(run(broker_end, binding, control, resolver, times, stopped));
    (helper_port, lookups, owner, helper, stop)
}

async fn harness(gateway: Arc<Gateway>, upstream_port: u16) -> Harness {
    let (helper_port, lookups, owner, _helper, stop) =
        start_channel(gateway.clone(), fixture_binding(), upstream_port).await;
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

fn fixture_destination(port: u16) -> OpenDestination {
    OpenDestination {
        opening_id: IdV1::random(),
        decision: EgressDecideRequestV1 {
            version: 1,
            decision_id: IdV1::random(),
            binding: fixture_binding(),
            flow_id: 1,
            authority: format!("{FIXTURE}:{port}"),
        },
        authority: Authority::new(FIXTURE, port, EgressTransport::ConnectTcpOpaque).unwrap(),
    }
}

fn fixture_opening_lifetime() -> (OpeningLifetime, OpeningOwner, ChannelStop) {
    let (owner, opening) = OpeningOwner::new();
    let (stop, running) = ChannelStop::new();
    (
        OpeningLifetime {
            owner: opening,
            stop: running,
        },
        owner,
        stop,
    )
}

#[test]
fn egress_broker_connect_and_invalidation_share_the_initial_poll() {
    for stop_channel in [false, true] {
        let (lifetime, opening, stop) = fixture_opening_lifetime();
        std::thread::scope(|scope| {
            let (announced, ready) = std::sync::mpsc::sync_channel(0);
            let (published, observed) = std::sync::mpsc::channel();
            let deadline = Instant::now() + Duration::from_secs(2);
            scope.spawn(move || {
                announced.send(()).unwrap();
                if stop_channel {
                    stop.stop();
                } else {
                    opening.retire(EgressCloseReasonV1::WorkerReset);
                }
                published.send(()).unwrap();
            });

            let polled = lifetime.poll_connect(deadline, || {
                ready.recv_timeout(Duration::from_secs(1)).unwrap();
                assert!(
                    matches!(
                        observed.recv_timeout(Duration::from_millis(150)),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ),
                    "invalidation must wait for the synchronous connect poll"
                );
                Poll::<()>::Pending
            });
            assert!(matches!(polled, Some(Poll::Pending)));
            observed.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(
                lifetime
                    .poll_connect(deadline, || -> Poll<()> {
                        panic!("a terminal opening must never poll a connect")
                    })
                    .is_none()
            );
        });
    }
}

#[tokio::test]
async fn egress_broker_waits_for_a_fresh_exact_numeric_permit_before_connecting() {
    for (reply, expected_accepts) in [
        (ConnectPermitReply::Fresh, 1),
        (ConnectPermitReply::Replayed, 0),
        (ConnectPermitReply::DifferentIp, 0),
        (ConnectPermitReply::Expired, 0),
        (ConnectPermitReply::Lost, 0),
    ] {
        let upstream = upstream().await;
        let gate = Arc::new(ObservationGate {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let gateway = Arc::new(Gateway {
            allow: AtomicBool::new(true),
            connect_permit_gate: Some(gate.clone()),
            connect_permit_reply: reply,
            ..Gateway::default()
        });
        let harness = harness(gateway.clone(), upstream.port).await;
        let authority = format!("{FIXTURE}:{}", upstream.port);
        let helper_port = harness.helper_port;
        let client = tokio::spawn(async move { connect(helper_port, &authority).await });
        timeout_at(
            Instant::now() + Duration::from_secs(3),
            gate.started.acquire(),
        )
        .await
        .expect("the broker must request the numeric permit")
        .unwrap()
        .forget();
        assert_eq!(upstream.accepts.load(Ordering::SeqCst), 0);
        let request = gateway.connect_permits.lock().unwrap()[0].clone();
        assert_eq!(request.ip.to_string(), "127.0.0.1");
        let decision = gateway.decisions.lock().unwrap()[0].clone();
        assert_eq!(request.decision_id, decision.decision_id);
        assert_eq!(request.binding, decision.binding);
        assert_eq!(request.flow_id, decision.flow_id);
        if matches!(reply, ConnectPermitReply::Expired) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        gate.release.add_permits(1);
        let (_client, response) = timeout_at(Instant::now() + Duration::from_secs(3), client)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.starts_with("HTTP/1.1 200"), expected_accepts == 1);
        assert_eq!(upstream.accepts.load(Ordering::SeqCst), expected_accepts);
        assert_eq!(gateway.connect_permits.lock().unwrap().len(), 1);
        harness.stop.stop();
        assert_eq!(harness.owner.await.unwrap().cause, ChannelExit::Stopped);
    }
}

#[tokio::test]
async fn egress_broker_an_unavailable_first_address_exhausts_the_permit() {
    let fallback = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = fallback.local_addr().unwrap().port();
    let first = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port));
    // Own the first loopback address on hosts that provide it, without
    // listening. On hosts without IPv6 loopback there is no listener either.
    let _first = tokio::net::TcpSocket::new_v6()
        .ok()
        .and_then(|socket| match socket.bind(first) {
            Ok(()) => Some(socket),
            Err(error) if error.kind() == std::io::ErrorKind::AddrNotAvailable => None,
            Err(error) => panic!("reserve first loopback address: {error}"),
        });
    let destination = fixture_destination(port);
    let resolver = Arc::new(
        Resolver::new(
            Arc::new(NoLookup(AtomicUsize::new(0))),
            vec![InternalService {
                authority: destination.authority.clone(),
                addresses: vec![first.ip(), fallback.local_addr().unwrap().ip()],
            }],
            vec![],
        )
        .unwrap(),
    );
    let control = gateway(true);
    let (lifetime, _opening, _stop) = fixture_opening_lifetime();
    let mut progress = ConnectProgress::default();

    let result = open_upstream(
        control.clone(),
        resolver,
        ChannelTimes {
            connect_attempt: Duration::from_millis(200),
            ..ChannelTimes::default()
        },
        destination,
        &lifetime,
        &mut progress,
    )
    .await;

    assert_eq!(control.decisions.lock().unwrap().len(), 1);
    let permits = control.connect_permits.lock().unwrap();
    assert_eq!(permits.len(), 1);
    assert_eq!(permits[0].ip, first.ip());
    assert!(!progress.pending);
    assert!(
        matches!(
            result,
            Err(Refusal {
                reason: EgressReasonV1::Unavailable,
                failure: Some(("connect_failed", "internal_service")),
            })
        ),
        "the unavailable first address must end the only authorized connect"
    );
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
    let (lifetime, _opening, _stop) = fixture_opening_lifetime();

    let result = open_upstream(
        control.clone(),
        resolver,
        ChannelTimes::default(),
        fixture_destination(443),
        &lifetime,
        &mut ConnectProgress::default(),
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
    let (lifetime, _opening, _stop) = fixture_opening_lifetime();

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        open_upstream(
            control,
            resolver,
            ChannelTimes::default(),
            fixture_destination(443),
            &lifetime,
            &mut ConnectProgress::default(),
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
    owner: tokio::task::JoinHandle<ChannelTermination>,
    blocked: Arc<tokio::sync::Notify>,
    stop: ChannelStop,
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
    let (stop, stopped) = ChannelStop::new();
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
        stop,
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
        stop: _stop,
    } = open_with_blocked_publication(gateway.clone(), upstream.port, complete_frames, 0).await;

    let deadline = Instant::now() + Duration::from_secs(2);
    timeout_at(deadline, blocked.notified()).await.unwrap();
    timeout_at(deadline, upstream.closed.notified())
        .await
        .expect("the upstream socket closes at receipt expiry, before the frame timeout");
    assert_eq!(
        timeout_at(deadline, owner).await.unwrap().unwrap().cause,
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
        stop: _stop,
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
        let socket = SocketControl::spawn(1, io, events, valid_until, None);
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
) -> (Owner<W>, mpsc::Receiver<Event>, ChannelStop) {
    let mut ledger = EgressChannelState::new(0);
    ledger
        .accept_capability(Some(&EgressCapabilityV1::default()), 0)
        .unwrap();
    let (events, incoming) = mpsc::channel(8);
    let (stop, stopping) = ChannelStop::new();
    let owner = Owner {
        output: Some(output),
        stop: stopping,
        stopped_during_write: false,
        binding: fixture_binding(),
        opening_id: IdV1::random(),
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
        observations_unconfirmed: 0,
        retirement_unconfirmed: false,
        renewing: false,
    };
    (owner, incoming, stop)
}

#[tokio::test]
async fn egress_broker_unconfirmed_prior_observation_leaves_no_audit_terminal() {
    let control = Arc::new(Gateway {
        prior_observation_reply: Some(ObservationReply::LostAfterCommit),
        ..Gateway::default()
    });
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        control.clone(),
        443,
        ChannelTimes::default(),
    );
    owner.record(EgressObservationV1::ChannelOpened {
        binding: owner.binding.clone(),
    });

    let terminal = owner
        .shutdown(ChannelExit::Stopped, Instant::now() + LOCAL_RETIREMENT_TIME)
        .await;

    assert_eq!(terminal.cause, ChannelExit::Stopped);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Unknown {
            observations_lost: 0,
            observations_unconfirmed: 1,
        }
    );
    assert!(
        matches!(
            control.observations.lock().unwrap().as_slice(),
            [EgressObservationV1::ChannelOpened { .. }]
        ),
        "a prior unconfirmed receipt must leave no terminal observation"
    );
}

#[tokio::test]
async fn egress_broker_audit_terminal_follows_the_saturated_observations() {
    let gate = Arc::new(ObservationGate {
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let control = Arc::new(Gateway {
        prior_observation_reply: Some(ObservationReply::Wait(gate.clone())),
        ..Gateway::default()
    });
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        control.clone(),
        443,
        ChannelTimes::default(),
    );
    for flow_id in 1..=OBSERVATION_SLOTS as u64 + 1 {
        owner.record(EgressObservationV1::Connection {
            binding: owner.binding.clone(),
            decision_id: IdV1::random(),
            flow_id,
            connected: true,
            failure: None,
            peer: Some("127.0.0.1:443".into()),
            resolver: "fixture".into(),
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut closing = std::pin::pin!(owner.shutdown(ChannelExit::Stopped, deadline));
    tokio::select! {
        biased;
        _ = &mut closing => panic!("shutdown awaits the prior submissions"),
        permits = timeout_at(deadline, gate.started.acquire_many(OBSERVATION_SLOTS as u32)) => {
            permits.unwrap().unwrap().forget();
        }
    }
    assert!(control.observations.lock().unwrap().is_empty());
    gate.release.add_permits(OBSERVATION_SLOTS);
    let terminal = timeout_at(deadline, closing).await.unwrap();

    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Acknowledged {
            observations_lost: 1
        }
    );
    let observations = control.observations.lock().unwrap();
    let (closing, prior) = observations.split_last().unwrap();
    assert_eq!(prior.len(), OBSERVATION_SLOTS);
    assert!(
        prior
            .iter()
            .all(|event| matches!(event, EgressObservationV1::Connection { .. }))
    );
    assert!(matches!(
        closing,
        EgressObservationV1::ChannelClosed {
            observations_lost: 1,
            ..
        }
    ));
}

#[tokio::test]
async fn egress_broker_missing_terminal_receipt_keeps_audit_unknown() {
    let control = Arc::new(Gateway {
        terminal_observation_reply: Some(ObservationReply::LostAfterCommit),
        ..Gateway::default()
    });
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        control.clone(),
        443,
        ChannelTimes::default(),
    );

    let terminal = owner
        .shutdown(
            ChannelExit::HelperClosed,
            Instant::now() + LOCAL_RETIREMENT_TIME,
        )
        .await;

    assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
    assert_eq!(terminal.cause, ChannelExit::HelperClosed);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Unknown {
            observations_lost: 0,
            observations_unconfirmed: 1,
        }
    );
    assert!(matches!(
        control.observations.lock().unwrap().as_slice(),
        [EgressObservationV1::ChannelClosed { reason, .. }] if reason == "helper_exit"
    ));
}

#[tokio::test]
async fn egress_broker_audit_cut_bounds_prior_and_terminal_acknowledgments() {
    for prior in [true, false] {
        let gate = Arc::new(ObservationGate {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let control = Arc::new(Gateway {
            prior_observation_reply: prior.then(|| ObservationReply::Wait(gate.clone())),
            terminal_observation_reply: (!prior).then(|| ObservationReply::Wait(gate.clone())),
            ..Gateway::default()
        });
        let (mut owner, _incoming, _stop) = fixture_owner(
            tokio::io::sink(),
            control.clone(),
            443,
            ChannelTimes::default(),
        );
        if prior {
            owner.record(EgressObservationV1::ChannelOpened {
                binding: owner.binding.clone(),
            });
        }
        let observations_until = Instant::now() + Duration::from_millis(30);
        let terminal_until = observations_until + Duration::from_millis(30);
        let mut audit = std::pin::pin!(owner.finish_audit(
            ChannelExit::HelperClosed,
            observations_until,
            terminal_until,
        ));
        if !prior {
            assert!(
                poll_fn(|cx| Poll::Ready(audit.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            sleep_until(terminal_until).await;
            gate.release.add_permits(1);
        }
        let result = audit.await;

        assert_eq!(gate.started.available_permits(), 1);
        assert!(
            Instant::now()
                >= if prior {
                    observations_until
                } else {
                    terminal_until
                }
        );
        assert_eq!(
            result,
            ChannelAuditClosure::Unknown {
                observations_lost: 0,
                observations_unconfirmed: 1,
            }
        );
        let observations = control.observations.lock().unwrap();
        if prior {
            assert!(observations.is_empty());
        } else {
            assert!(
                matches!(
                    observations.as_slice(),
                    [EgressObservationV1::ChannelClosed { .. }]
                ),
                "the fixture committed the terminal before returning its late receipt"
            );
        }
    }
}

#[tokio::test]
async fn egress_broker_missing_detached_opening_report_keeps_quiescence_unknown() {
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        gateway(true),
        443,
        ChannelTimes::default(),
    );
    let now = owner.now_ms();
    owner
        .ledger
        .on_worker(
            &EgressMessageV1::Open {
                flow_id: 1,
                authority: format!("{FIXTURE}:443"),
            },
            now,
        )
        .unwrap();
    let (opening, _lifetime) = OpeningOwner::new();
    let (reported, outcome) = oneshot::channel();
    let mut flow = owner.pending_flow(IdV1::random());
    flow.opening = Some(OpeningHandle {
        owner: opening,
        outcome,
    });
    owner.flows.insert(1, flow);
    drop(reported);

    assert!(
        owner
            .worker_frame(EgressMessageV1::Reset {
                flow_id: 1,
                reason: EgressReasonV1::Cancelled,
            })
            .await
            .is_err()
    );
    assert!(owner.flows.is_empty());

    let terminal = owner
        .shutdown(
            ChannelExit::Protocol,
            Instant::now() + LOCAL_RETIREMENT_TIME,
        )
        .await;
    assert_eq!(terminal.cause, ChannelExit::Protocol);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Unknown);
}

#[tokio::test]
async fn egress_broker_ready_retirement_receipts_after_deadline_cannot_confirm_quiescence() {
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        gateway(true),
        443,
        ChannelTimes::default(),
    );
    let expired = Instant::now();
    let (reported_opening, outcome) = oneshot::channel();
    assert!(
        reported_opening
            .send(OpeningOutcome {
                result: Err(cancelled_opening()),
                socket_unobserved: false
            })
            .is_ok()
    );
    let (reported_socket, closed) = oneshot::channel();
    assert!(
        reported_socket
            .send(SocketClosure {
                at: Instant::now(),
                reason: EgressCloseReasonV1::Revoked,
                traffic: SocketTraffic::default(),
            })
            .is_ok()
    );

    let opening = owner
        .collect_retired(
            RetiredFlow::Opening {
                flow_id: 1,
                decision_id: IdV1::random(),
                reason: EgressCloseReasonV1::OwnerExit,
                outcome,
            },
            expired,
        )
        .await;
    let socket = owner
        .record_closed(
            ClosingFlow {
                flow_id: 2,
                decision_id: IdV1::random(),
                connected_at: expired,
                closed,
            },
            expired,
        )
        .await;

    assert_eq!(
        (opening, socket),
        (Err(()), Err(())),
        "ready receipts beyond the budget are late"
    );
    let terminal = owner
        .shutdown(
            ChannelExit::HelperClosed,
            Instant::now() + LOCAL_RETIREMENT_TIME,
        )
        .await;
    assert_eq!(terminal.cause, ChannelExit::HelperClosed);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Unknown);
}

#[tokio::test]
async fn egress_broker_eof_without_socket_confirmation_keeps_quiescence_unknown() {
    let control = gateway(true);
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        control.clone(),
        443,
        ChannelTimes::default(),
    );
    let deadline = Instant::now() + Duration::from_millis(30);
    let (authority, signalled) = watch::channel(SocketAuthority::default());
    let (commands, _incoming_commands) = mpsc::channel(1);
    let (reported, closed) = oneshot::channel();
    let mut flow = owner.pending_flow(IdV1::random());
    flow.socket = Some(SocketControl {
        authority: Arc::new(authority),
        commands,
        closed,
        valid_until: deadline,
    });
    owner.flows.insert(1, flow);

    let terminal = owner.shutdown(ChannelExit::HelperClosed, deadline).await;

    assert!(Instant::now() >= deadline);
    assert_eq!(
        signalled.borrow().stop.map(|(_, reason)| reason),
        Some(EgressCloseReasonV1::OwnerExit)
    );
    assert!(reported.is_closed());
    assert_eq!(terminal.cause, ChannelExit::HelperClosed);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Unknown);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Unknown {
            observations_lost: 1,
            observations_unconfirmed: 0,
        }
    );
    assert!(control.observations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn egress_broker_stop_cancels_a_pending_handshake() {
    for waits_for_ready in [false, true] {
        let (broker, mut helper) = tokio::io::duplex(if waits_for_ready { 4096 } else { 1 });
        let control = Arc::new(Gateway::default());
        let resolver = Arc::new(
            Resolver::new(Arc::new(NoLookup(AtomicUsize::new(0))), vec![], vec![]).unwrap(),
        );
        let (stop, stopped) = ChannelStop::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let ((), terminal) = tokio::join!(
            async {
                if waits_for_ready {
                    channel_protocol::read_handshake(&mut helper, deadline)
                        .await
                        .unwrap();
                } else {
                    // A one-byte pipe keeps the rest of HELLO blocked.
                    timeout_at(deadline, helper.read_u8())
                        .await
                        .unwrap()
                        .unwrap();
                }
                stop.stop();
            },
            timeout_at(
                deadline,
                run(
                    broker,
                    fixture_binding(),
                    control.clone(),
                    resolver,
                    ChannelTimes::default(),
                    stopped,
                ),
            ),
        );
        let terminal = terminal.expect("STOP must interrupt the handshake");
        assert_eq!(terminal.cause, ChannelExit::Stopped);
        assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
        assert_eq!(terminal.audit, ChannelAuditClosure::NotOpened);
        assert!(control.observations.lock().unwrap().is_empty());
        assert!(control.decisions.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn egress_broker_prepares_before_open_and_requires_fresh_correlated_receipt() {
    for (opening_reply, stop_with_ack, expected_refusal) in [
        (OpeningReply::Fresh, false, None),
        (
            OpeningReply::Replayed,
            false,
            Some(ChannelExit::OpeningReplayed),
        ),
        (
            OpeningReply::DifferentBinding,
            false,
            Some(ChannelExit::AuditUnavailable),
        ),
        (OpeningReply::Fresh, true, Some(ChannelExit::Stopped)),
    ] {
        let (broker, mut helper) = tokio::io::duplex(4096);
        let (stop, stopped) = ChannelStop::new();
        let (stop_on_opening_ack, _stop_owner) = if stop_with_ack {
            (Some(stop), None)
        } else {
            (None, Some(stop))
        };
        let control = Arc::new(Gateway {
            allow: AtomicBool::new(true),
            opening_reply,
            stop_on_opening_ack,
            ..Gateway::default()
        });
        let binding = fixture_binding();
        let deadline = Instant::now() + Duration::from_secs(2);
        let (prepared, ()) = tokio::join!(
            timeout_at(deadline, prepare(broker, binding.clone(), stopped)),
            async {
                let hello = channel_protocol::read_handshake(&mut helper, deadline)
                    .await
                    .unwrap();
                channel_protocol::write_handshake(&mut helper, &hello, deadline)
                    .await
                    .unwrap();
            },
        );
        let prepared = prepared.unwrap().unwrap();
        let request = prepared.opening_request().clone();
        assert_eq!(request.binding, binding);
        assert_eq!(control.opening_calls.load(Ordering::SeqCst), 0);
        channel_protocol::write_message(
            &mut helper,
            &EgressMessageV1::Open {
                flow_id: 1,
                authority: format!("{FIXTURE}:443"),
            },
            deadline,
        )
        .await
        .unwrap();
        let result = timeout_at(deadline, prepared.open(control.as_ref()))
            .await
            .unwrap();
        match (result, expected_refusal) {
            (Ok(opened), None) => {
                assert_eq!(opened.receipt().binding, binding);
                assert_eq!(opened.receipt().observation_id, request.observation_id);
                assert_eq!(
                    opened.receipt().disposition,
                    EgressChannelOpeningDispositionV1::Fresh
                );
            }
            (Err(terminal), Some(cause)) => {
                assert_eq!(terminal.cause, cause);
                assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
                if stop_with_ack {
                    assert_eq!(terminal.audit, ChannelAuditClosure::OpeningAcknowledged);
                }
            }
            _ => panic!("unexpected channel opening result"),
        }
        assert_eq!(control.opening_calls.load(Ordering::SeqCst), 1);
        assert!(control.decisions.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn egress_broker_stop_during_opening_ack_keeps_audit_unknown() {
    let (broker, mut helper) = tokio::io::duplex(4096);
    let pending = Arc::new(PendingOperation::default());
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        prior_observation_reply: Some(ObservationReply::WaitAfterCommit(pending.clone())),
        ..Gateway::default()
    });
    let lookups = Arc::new(NoLookup(AtomicUsize::new(0)));
    let resolver = Arc::new(Resolver::new(lookups.clone(), vec![], vec![]).unwrap());
    let (stop, stopped) = ChannelStop::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let ((), terminal) = tokio::join!(
        async {
            let hello = channel_protocol::read_handshake(&mut helper, deadline)
                .await
                .unwrap();
            channel_protocol::write_handshake(&mut helper, &hello, deadline)
                .await
                .unwrap();
            timeout_at(deadline, pending.started.notified())
                .await
                .unwrap();
            channel_protocol::write_message(
                &mut helper,
                &EgressMessageV1::Open {
                    flow_id: 1,
                    authority: format!("{FIXTURE}:443"),
                },
                deadline,
            )
            .await
            .unwrap();
            stop.stop();
        },
        timeout_at(
            deadline,
            run(
                broker,
                fixture_binding(),
                control.clone(),
                resolver,
                ChannelTimes::default(),
                stopped,
            ),
        ),
    );

    let terminal = terminal.expect("STOP must interrupt the opening ACK wait");
    assert_eq!(terminal.cause, ChannelExit::Stopped);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Unknown {
            observations_lost: 0,
            observations_unconfirmed: 1,
        }
    );
    timeout_at(deadline, pending.dropped.notified())
        .await
        .unwrap();
    assert!(matches!(
        control.observations.lock().unwrap().as_slice(),
        [EgressObservationV1::ChannelOpened { .. }]
    ));
    assert!(control.decisions.lock().unwrap().is_empty());
    assert_eq!(lookups.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn egress_broker_clean_eof_reports_local_quiescence_and_its_cause() {
    let (broker, mut helper) = tokio::io::duplex(4096);
    let (_stop, stopped) = ChannelStop::new();
    let owner = tokio::spawn(run(
        broker,
        fixture_binding(),
        gateway(true),
        fixture_resolver(443, Arc::new(NoLookup(AtomicUsize::new(0)))),
        ChannelTimes::default(),
        stopped,
    ));
    let deadline = Instant::now() + Duration::from_secs(2);
    let hello = channel_protocol::read_handshake(&mut helper, deadline)
        .await
        .unwrap();
    channel_protocol::write_handshake(&mut helper, &hello, deadline)
        .await
        .unwrap();
    drop(helper);

    let terminal = timeout_at(deadline, owner).await.unwrap().unwrap();
    assert_eq!(terminal.cause, ChannelExit::HelperClosed);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Acknowledged {
            observations_lost: 0
        }
    );
}

#[tokio::test]
async fn egress_broker_stop_cancels_an_opening_waiting_for_a_decision() {
    let upstream = upstream().await;
    let pending = Arc::new(PendingOperation::default());
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        decision_hold: Some(pending.clone()),
        ..Gateway::default()
    });
    let channel = open_with_blocked_publication(control.clone(), upstream.port, 3, 0).await;
    let deadline = Instant::now() + Duration::from_secs(2);
    timeout_at(deadline, pending.started.notified())
        .await
        .unwrap();
    channel.stop.stop();

    timeout_at(deadline, pending.dropped.notified())
        .await
        .expect("Stop drops the in-flight decision wait");
    assert_eq!(
        timeout_at(deadline, channel.owner)
            .await
            .unwrap()
            .unwrap()
            .cause,
        ChannelExit::Stopped
    );
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 0);
    assert!(
        !control
            .observations
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, EgressObservationV1::Connection { .. }))
    );
}

#[tokio::test]
async fn egress_broker_flow_retirement_cancels_pending_resolution() {
    let upstream = upstream().await;
    let pending = Arc::new(PendingOperation::default());
    let control = gateway(true);
    let (mut owner, _incoming, _stop) = fixture_owner(
        tokio::io::sink(),
        control.clone(),
        upstream.port,
        ChannelTimes::default(),
    );
    owner.resolver =
        Arc::new(Resolver::new(Arc::new(HeldLookup(pending.clone())), vec![], vec![]).unwrap());
    let deadline = Instant::now() + Duration::from_secs(2);
    owner
        .worker_frame(EgressMessageV1::Open {
            flow_id: 1,
            authority: format!("{FIXTURE}:{}", upstream.port),
        })
        .await
        .unwrap();
    timeout_at(deadline, pending.started.notified())
        .await
        .unwrap();

    owner
        .worker_frame(EgressMessageV1::Reset {
            flow_id: 1,
            reason: EgressReasonV1::Cancelled,
        })
        .await
        .unwrap();
    timeout_at(deadline, pending.dropped.notified())
        .await
        .expect("retirement drops the in-flight resolver wait");
    assert_eq!(upstream.accepts.load(Ordering::SeqCst), 0);
    owner
        .shutdown(ChannelExit::Stopped, Instant::now() + LOCAL_RETIREMENT_TIME)
        .await;
    assert!(
        !control
            .observations
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, EgressObservationV1::Connection { .. }))
    );
}

#[tokio::test]
async fn egress_broker_retirement_closes_observed_socket_before_handoff() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let control = gateway(true);
    let (output, mut helper) = tokio::io::duplex(4096);
    let (mut owner, mut incoming, _stop) =
        fixture_owner(output, control.clone(), port, ChannelTimes::default());
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
    let queued = timeout_at(deadline, incoming.recv())
        .await
        .unwrap()
        .unwrap();

    owner
        .worker_frame(EgressMessageV1::Reset {
            flow_id: 1,
            reason: EgressReasonV1::Cancelled,
        })
        .await
        .unwrap();
    assert_eq!(
        timeout_at(deadline, upstream.read(&mut [0; 1]))
            .await
            .expect("retirement closes the observed socket before consuming its wakeup")
            .unwrap(),
        0
    );
    let newer = IdV1::random();
    let (new_owner, _opened) = OpeningOwner::new();
    let (_reported, outcome) = oneshot::channel();
    let mut replacement = owner.pending_flow(newer.clone());
    replacement.opening = Some(OpeningHandle {
        owner: new_owner,
        outcome,
    });
    owner.flows.insert(1, replacement);
    owner.event(queued).await.unwrap();
    let flow = owner
        .flows
        .remove(&1)
        .expect("the newer generation remains");
    assert!(flow.decision_id == newer && flow.opening.is_some());
    drop(owner.output.take());
    assert!(
        EgressFrameReader::new()
            .read(&mut helper)
            .await
            .unwrap()
            .is_none()
    );
    owner
        .shutdown(ChannelExit::Stopped, Instant::now() + LOCAL_RETIREMENT_TIME)
        .await;
    let observations = control.observations.lock().unwrap();
    assert_eq!(
        observations
            .iter()
            .filter(|event| matches!(
                event,
                EgressObservationV1::Connection {
                    flow_id: 1,
                    connected: true,
                    ..
                }
            ))
            .count(),
        1
    );
    assert_eq!(
        observations
            .iter()
            .filter(|event| matches!(
                event,
                EgressObservationV1::Closed {
                    flow_id: 1,
                    reason: EgressCloseReasonV1::WorkerReset,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn egress_broker_connect_audit_requires_known_outcome_through_eof() {
    enum Completion {
        SocketUnavailable,
        OpaqueError,
        Connected,
    }
    for completion in [
        Completion::SocketUnavailable,
        Completion::OpaqueError,
        Completion::Connected,
    ] {
        let control = gateway(true);
        let (mut owner, _incoming, _stop) = fixture_owner(
            tokio::io::sink(),
            control.clone(),
            443,
            ChannelTimes::default(),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let (stream, mut peer) = tokio::io::duplex(64);
        let (result, expected_connection, lost) = match completion {
            Completion::SocketUnavailable => (None, Some(false), 0),
            Completion::OpaqueError => (
                Some(Err(std::io::Error::from(
                    std::io::ErrorKind::ConnectionRefused,
                ))),
                None,
                1,
            ),
            Completion::Connected => (
                Some(Ok(PendingOpen {
                    socket: SocketControl::spawn(1, stream, owner.events.clone(), deadline, None),
                    peer: "127.0.0.1:443".parse().unwrap(),
                    connected_at: Instant::now(),
                    resolver: "fixture",
                })),
                Some(true),
                0,
            ),
        };
        let mut progress = ConnectProgress {
            pending: true,
            ..ConnectProgress::default()
        };
        let result = progress.complete(result).ok_or(Refusal {
            reason: EgressReasonV1::Unavailable,
            failure: Some(("connect_failed", "fixture")),
        });
        let (reported, outcome) = oneshot::channel();
        assert!(
            reported
                .send(OpeningOutcome {
                    result,
                    socket_unobserved: progress.pending || progress.unobserved,
                })
                .is_ok()
        );
        let (opening, _lifetime) = OpeningOwner::new();
        let mut flow = owner.pending_flow(IdV1::random());
        flow.opening = Some(OpeningHandle {
            owner: opening,
            outcome,
        });
        owner.flows.insert(1, flow);

        let terminal = owner.shutdown(ChannelExit::HelperClosed, deadline).await;

        assert_eq!(terminal.cause, ChannelExit::HelperClosed);
        assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
        if matches!(completion, Completion::Connected) {
            assert_eq!(
                timeout_at(deadline, peer.read(&mut [0; 1]))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        }
        let observations = control.observations.lock().unwrap();
        let connections: Vec<_> = observations
            .iter()
            .filter_map(|event| match event {
                EgressObservationV1::Connection { connected, .. } => Some(*connected),
                _ => None,
            })
            .collect();
        assert_eq!(
            connections,
            expected_connection.as_slice(),
            "only an outcome with known provenance is observed"
        );
        assert!(observations.iter().any(|event| matches!(
            event, EgressObservationV1::ChannelClosed { observations_lost, .. } if *observations_lost == lost
        )));
        assert_eq!(
            observations.iter().any(|event| matches!(
                event,
                EgressObservationV1::Closed {
                    reason: EgressCloseReasonV1::OwnerExit,
                    ..
                }
            )),
            matches!(completion, Completion::Connected)
        );
    }
}

#[tokio::test]
async fn egress_broker_receipt_expiry_closes_socket_before_event_consumption() {
    let upstream = upstream().await;
    let gateway = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(300),
        ..Gateway::default()
    });
    let (output, mut helper) = tokio::io::duplex(4096);
    let (mut owner, mut incoming, _stop) = fixture_owner(
        output,
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
    assert!(matches!(
        timeout_at(deadline, EgressFrameReader::new().read(&mut helper))
            .await
            .unwrap()
            .unwrap(),
        Some(EgressMessageV1::Rejected {
            flow_id: 1,
            reason: EgressReasonV1::Expired
        })
    ));
    assert!(owner.flows.is_empty());
    owner
        .shutdown(ChannelExit::Stopped, Instant::now() + LOCAL_RETIREMENT_TIME)
        .await;
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
        let (mut owner, mut incoming, _stop) = fixture_owner(output, control.clone(), port, times);
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
        owner
            .shutdown(ChannelExit::Stopped, Instant::now() + LOCAL_RETIREMENT_TIME)
            .await;
        assert!(control.observations.lock().unwrap().iter().any(|event| matches!(event,
            EgressObservationV1::Closed { flow_id: 1, reason: observed, bytes_down: 3, .. } if *observed == reason)));
    }
}

async fn open_for_partial_data(
    control: Arc<Gateway>,
) -> (BlockedHarness, TcpStream, EgressFrameReader) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut channel = open_with_blocked_publication(control, port, 3, 2).await;
    let deadline = Instant::now() + Duration::from_secs(2);
    let (upstream, _) = timeout_at(deadline, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut frames = EgressFrameReader::new();
    assert!(matches!(
        frames.read(&mut channel.helper).await.unwrap(),
        Some(EgressMessageV1::Accepted { flow_id: 1 })
    ));
    assert!(matches!(
        frames.read(&mut channel.helper).await.unwrap(),
        Some(EgressMessageV1::Credit { flow_id: 1, .. })
    ));
    (channel, upstream, frames)
}

async fn block_data_frame(channel: &mut BlockedHarness, upstream: &mut TcpStream) -> Instant {
    let deadline = Instant::now() + Duration::from_secs(2);
    channel_protocol::write_message(
        &mut channel.helper,
        &EgressMessageV1::Credit {
            flow_id: 1,
            bytes: 32,
        },
        deadline,
    )
    .await
    .unwrap();
    upstream.write_all(b"data").await.unwrap();
    timeout_at(deadline, channel.blocked.notified())
        .await
        .expect("the writer is pending after emitting a partial data frame");
    deadline
}

#[tokio::test]
async fn egress_broker_expiry_during_partial_data_frame_closes_the_helper_channel() {
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        remaining_ms: Some(300),
        ..Gateway::default()
    });
    let (mut channel, mut upstream, mut frames) = open_for_partial_data(control.clone()).await;
    let deadline = block_data_frame(&mut channel, &mut upstream).await;
    assert_eq!(
        timeout_at(deadline, channel.owner)
            .await
            .expect("the data deadline closes the helper channel")
            .unwrap()
            .cause,
        ChannelExit::Protocol,
    );
    assert!(matches!(
        frames.read(&mut channel.helper).await,
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
async fn egress_broker_stop_interrupts_partial_helper_publication() {
    let control = gateway(true);
    let (mut channel, mut upstream, mut frames) = open_for_partial_data(control.clone()).await;
    let deadline = block_data_frame(&mut channel, &mut upstream).await;
    channel.stop.stop();

    let count = timeout_at(deadline, upstream.read(&mut [0; 1]))
        .await
        .expect("Stop closes the socket while helper publication is blocked")
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        timeout_at(deadline, channel.owner)
            .await
            .unwrap()
            .unwrap()
            .cause,
        ChannelExit::Stopped
    );
    assert!(matches!(
        frames.read(&mut channel.helper).await,
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
                    reason: EgressCloseReasonV1::OwnerExit,
                    bytes_down: 4,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn egress_broker_received_revocation_interrupts_partial_helper_publication() {
    let renewal = Arc::new(ScriptedRenewal::default());
    let control = Arc::new(Gateway {
        allow: AtomicBool::new(true),
        renewal: Some(renewal.clone()),
        ..Gateway::default()
    });
    let (mut channel, mut upstream, mut frames) = open_for_partial_data(control.clone()).await;
    tokio::time::timeout(Duration::from_secs(4), renewal.requested.notified())
        .await
        .expect("the renewal is in flight before the helper write");
    let deadline = block_data_frame(&mut channel, &mut upstream).await;
    let id = control.decisions.lock().unwrap()[0].decision_id.clone();
    control.revoke.lock().unwrap().push(id);
    let awaiting_answer = upstream.try_read(&mut [0; 1]).unwrap_err();
    assert_eq!(awaiting_answer.kind(), std::io::ErrorKind::WouldBlock);
    renewal.release.notify_one();
    timeout_at(deadline, renewal.answered.notified())
        .await
        .expect("the gateway returns the revocation");

    let count = timeout_at(deadline, upstream.read(&mut [0; 1]))
        .await
        .expect("the received revocation closes the socket while helper publication is blocked")
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        timeout_at(deadline, channel.owner)
            .await
            .unwrap()
            .unwrap()
            .cause,
        ChannelExit::Protocol
    );
    assert!(matches!(
        frames.read(&mut channel.helper).await,
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
                    reason: EgressCloseReasonV1::Revoked,
                    bytes_down: 4,
                    ..
                }
            ))
    );
}

#[test]
fn egress_broker_renewal_evidence_is_bound_to_the_exact_request() {
    let id = IdV1::random();
    let requested = std::slice::from_ref(&id);
    let mut answer = EgressRenewResponseV1 {
        version: 1,
        revoked: vec![id.clone()],
        expired: vec![],
    };
    assert!(valid_renewal(&answer, requested));
    answer.version = 2;
    assert!(!valid_renewal(&answer, requested));
    answer.version = 1;
    answer.expired.push(id.clone());
    assert!(!valid_renewal(&answer, requested));
    answer.revoked.clear();
    assert!(!valid_renewal(&answer, &[IdV1::random()]));
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
    harness.stop.stop();
    assert_eq!(harness.owner.await.unwrap().cause, ChannelExit::Stopped);
}

#[tokio::test]
async fn egress_broker_committed_sqlite_decision_does_not_authorize_loopback() {
    use crate::{
        api::{GatewayState, TenantGateway, gateway_router},
        audit::egress_decisions::tests::loopback_store_fixture,
        model::Event,
        policy::Policy,
        redaction::Redactor,
        runner::{AuditClient, egress_control::GatewayEgressControl},
    };
    use axum::{
        extract::Request,
        middleware::{self, Next},
    };

    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let authority = format!("{FIXTURE}:{port}");
    let (directory, store, binding) = loopback_store_fixture(&authority);
    // This reader has its own SQLite connection and never calls the gateway's Store.
    let reader = rusqlite::Connection::open_with_flags(
        directory.path().join("tenant-a.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let read_events = || {
        let mut query = reader
            .prepare(
                "SELECT seq,payload FROM events WHERE kind LIKE 'egress.%' \
                 AND json_extract(payload,'$.session_id')=?1 ORDER BY seq LIMIT 16",
            )
            .unwrap();
        query
            .query_map([&binding.session_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .map(|row| {
                let (seq, payload) = row.unwrap();
                (seq, serde_json::from_str::<Event>(&payload).unwrap())
            })
            .collect::<Vec<_>>()
    };
    let token = IdV1::random().as_str().to_owned();
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: store.clone(),
            runner_token: token.clone(),
            viewer_token: IdV1::random().as_str().to_owned(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let opening_requests = Arc::new(AtomicUsize::new(0));
    let rejected_permits = Arc::new(AtomicUsize::new(0));
    let committed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let router = gateway_router(state).layer(middleware::from_fn({
        let opening_requests = opening_requests.clone();
        let rejected_permits = rejected_permits.clone();
        let committed = committed.clone();
        let release = release.clone();
        move |request: Request, next: Next| {
            let opening_requests = opening_requests.clone();
            let rejected_permits = rejected_permits.clone();
            let committed = committed.clone();
            let release = release.clone();
            async move {
                if request.uri().path() == "/api/v1/egress/channel-openings" {
                    opening_requests.fetch_add(1, Ordering::SeqCst);
                }
                let decision = request.uri().path() == "/api/v1/egress/decisions";
                let permit = request.uri().path() == "/api/v1/egress/connect-permits";
                let response = next.run(request).await;
                if permit && response.status() == axum::http::StatusCode::BAD_REQUEST {
                    rejected_permits.fetch_add(1, Ordering::SeqCst);
                }
                if decision {
                    committed.notify_one();
                    release.notified().await;
                }
                response
            }
        }
    }));
    let gateway = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let control = Arc::new(GatewayEgressControl::new(
        AuditClient::new(&origin, &token).unwrap(),
    ));
    let (helper_port, lookups, owner, helper, stop) =
        start_channel(control, binding.clone(), port).await;
    let request_authority = authority.clone();
    let client = tokio::spawn(async move { connect(helper_port, &request_authority).await });

    timeout_at(
        Instant::now() + Duration::from_secs(3),
        committed.notified(),
    )
    .await
    .unwrap();
    assert_eq!(opening_requests.load(Ordering::SeqCst), 1);
    let events = read_events();
    let decisions: Vec<_> = events
        .iter()
        .filter(|(_, event)| event.kind == "egress.decision")
        .collect();
    assert_eq!(decisions.len(), 1);
    let (decision_seq, decision) = decisions[0];
    let request: EgressDecideRequestV1 =
        serde_json::from_value(decision.data["request"].clone()).unwrap();
    assert_eq!(request.binding, binding);
    assert_eq!(request.authority, authority);
    assert_eq!(decision.id, request.decision_id.as_str());
    assert_eq!(decision.data["decision"]["outcome"], "allow");
    assert!(
        timeout_at(
            Instant::now() + Duration::from_millis(100),
            upstream.accept()
        )
        .await
        .is_err(),
        "the listener accepted a connection while the committed reply was held"
    );
    release.notify_one();

    let (_client, response) = timeout_at(Instant::now() + Duration::from_secs(3), client)
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 403"));
    assert_eq!(rejected_permits.load(Ordering::SeqCst), 1);
    stop.stop();
    let terminal = timeout_at(Instant::now() + Duration::from_secs(7), owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal.cause, ChannelExit::Stopped);
    assert_eq!(terminal.local_quiescence, LocalQuiescence::Confirmed);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Acknowledged {
            observations_lost: 0
        }
    );
    timeout_at(Instant::now() + Duration::from_secs(2), helper)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let events = read_events();
    assert!(!events.iter().any(|(_, event)| {
        event.kind == "egress.connect.permit" || event.kind == "egress.connection"
    }));
    let (closed_seq, closed) = events.last().unwrap();
    assert!(*closed_seq > *decision_seq && closed.kind == "egress.channel.closed");
    assert_eq!(closed.data["observations_lost"], 0);
    assert_eq!(
        closed.data["binding"],
        serde_json::to_value(&binding).unwrap()
    );
    assert!(store.integrity().unwrap().valid);
    assert_eq!(lookups.0.load(Ordering::SeqCst), 0);
    assert!(
        timeout_at(
            Instant::now() + Duration::from_millis(100),
            upstream.accept()
        )
        .await
        .is_err(),
        "the rejected loopback permit must leave the upstream unopened"
    );
    gateway.abort();
    assert!(gateway.await.unwrap_err().is_cancelled());
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
    harness.stop.stop();
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
    harness.stop.stop();
}

#[tokio::test]
async fn egress_broker_refuses_every_flow_when_the_channel_cannot_be_recorded() {
    let upstream = upstream().await;
    let unrecorded = gateway(true);
    unrecorded.observe_unavailable.store(true, Ordering::SeqCst);
    let harness = harness(unrecorded, upstream.port).await;
    let terminal = harness.owner.await.unwrap();
    assert_eq!(terminal.cause, ChannelExit::AuditUnavailable);
    assert_eq!(
        terminal.audit,
        ChannelAuditClosure::Unknown {
            observations_lost: 0,
            observations_unconfirmed: 1,
        }
    );
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
    harness.stop.stop();
}
