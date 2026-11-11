//! Private control-side owner of an already supplied non-TTY byte stream.
//!
//! There is intentionally no production constructor or authority supplied by a
//! caller/frame. `current_admission` refuses every production dispatch until the
//! exact-runtime permit, current binding, RoleLease and mandatory audit gate are
//! integrated. This source segment alone cannot open or authorize an exec.
//! The software tests supply an explicitly synthetic admission, real OS sockets,
//! the real HTTP client and durable private business files.
use super::{
    ChannelState, CloseReasonV1, CloseV1, ErrorCodeV1, FrameLimit, FrameV1, HandshakeV1,
    MAX_CONTROL_FRAME_BYTES, MAX_REQUEST_FRAME_BYTES, ReplyV1, RequestPayloadV1, RequestV1,
    ResponseResultV1, ResponseV1,
};
use crate::{
    control_fs::{Directory, PrivateFile},
    relay::{
        RelayEndpointV1,
        client::{RelayClientError, WorkerRelayClient},
        validation,
    },
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::{Instant, timeout_at},
};
mod outbox;
use outbox::Outbox;
mod io_owner;
use io_owner::{IoCommand, IoOwner};

const FRAME_TIME: Duration = Duration::from_secs(5);
const IDLE_TIME: Duration = Duration::from_secs(2);
const OPERATION_TIME: Duration = Duration::from_secs(45);
const EXCHANGE_BYTES: u32 = 3 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlError {
    InvalidRequest,
    Binding,
    Conflict,
    Busy,
    Capacity,
    Persistence,
    AuthorityUnavailable,
    Protocol,
    Transport,
    Timeout,
    PeerClosed,
    ExtraFrame,
    NeedsReconciliation,
}

/// No proof or human identity. Endpoint and credential ID must come from the
/// current workload binding retained by control, not from a frame/worker label.
#[derive(Clone)]
struct Binding {
    endpoint: RelayEndpointV1,
    credential_id: String,
}

/// This list remains in the control memory only. It is not exhaustive DLP.
struct Secrets(Vec<String>);
impl Secrets {
    fn check(&self, bytes: &[u8]) -> Result<(), ControlError> {
        if self.0.is_empty()
            || self.0.len() > 64
            || self
                .0
                .iter()
                .any(|secret| secret.is_empty() || secret.len() > 4096)
        {
            return Err(ControlError::InvalidRequest);
        }
        for secret in &self.0 {
            if bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
            {
                return Err(ControlError::InvalidRequest);
            }
            validation::decode::<serde_json::Value>(bytes, Some(secret))
                .map_err(|_| ControlError::InvalidRequest)?;
        }
        Ok(())
    }
}

/// The service must supply shared tenant/global counters, never per-channel
/// replacements. The byte-denominated permits are logical admission units;
/// decoded DTOs, JSON values and encoding copies can coexist above this size.
/// Frame/DTO bounds remain separate. Total live memory/RSS is not measured here.
struct ByteBudget {
    tenant: Arc<Semaphore>,
    global: Arc<Semaphore>,
}
struct Reservation {
    _tenant: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}
impl ByteBudget {
    fn reserve(&self) -> Result<Reservation, ControlError> {
        let tenant = self
            .tenant
            .clone()
            .try_acquire_many_owned(EXCHANGE_BYTES)
            .map_err(|_| ControlError::Capacity)?;
        let global = self
            .global
            .clone()
            .try_acquire_many_owned(EXCHANGE_BYTES)
            .map_err(|_| ControlError::Capacity)?;
        Ok(Reservation {
            _tenant: tenant,
            _global: global,
        })
    }
}

/// The future borrows a registry-retained owner. IO handles remain inside that
/// retention after cancellation; a timeout cannot silently reopen the journal.
/// Runtime child/stderr ownership is deliberately not supplied by this segment.
struct ControlOwner {
    hello: HandshakeV1,
    binding: Arc<Binding>,
    client: Arc<WorkerRelayClient>,
    secrets: Arc<Secrets>,
    io: Arc<IoOwner>,
    bytes: ByteBudget,
    expires: Instant,
    #[cfg(test)]
    synthetic_admission: Arc<std::sync::atomic::AtomicBool>,
}
impl ControlOwner {
    async fn current_admission(&self) -> Result<(), ControlError> {
        // This is a closed integration point, not a caller-provided predicate or
        // a conversion of a consumed start permit into an exec permission.
        #[cfg(test)]
        if self
            .synthetic_admission
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        Err(ControlError::AuthorityUnavailable)
    }

    async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &mut self,
        mut input: R,
        mut output: W,
        mut shutdown: oneshot::Receiver<CloseReasonV1>,
    ) -> Result<(), ControlError> {
        let expires = self.expires;
        let (result, reason) = tokio::select! {
            biased;
            reason = &mut shutdown => (Ok(()), reason.unwrap_or(CloseReasonV1::Shutdown)),
            _ = tokio::time::sleep_until(expires) => (Err(ControlError::Timeout), CloseReasonV1::Expired),
            result = self.run(&mut input, &mut output) => {
                let reason = match result {
                    Ok(()) => CloseReasonV1::Shutdown,
                    Err(ControlError::Timeout) => CloseReasonV1::Timeout,
                    Err(ControlError::Capacity) => CloseReasonV1::Capacity,
                    Err(ControlError::Persistence | ControlError::NeedsReconciliation) => CloseReasonV1::AuditUnavailable,
                    Err(ControlError::AuthorityUnavailable | ControlError::Binding) => CloseReasonV1::IdentityChanged,
                    Err(ControlError::Transport | ControlError::PeerClosed) => CloseReasonV1::Transport,
                    _ => CloseReasonV1::Protocol,
                };
                (result, reason)
            }
        };
        // Best effort, bounded by the same frame deadline. Local EOF/close is
        // never a worker terminal, quota release or cancellation at the server.
        let closing_deadline = Instant::now() + FRAME_TIME;
        let close = FrameV1::Close(CloseV1 {
            version: 1,
            channel_id: self.hello.channel_id.clone(),
            epoch: self.hello.epoch,
            reason,
        });
        let _ = timeout_at(
            closing_deadline,
            checked_write(&mut output, &close, &self.secrets),
        )
        .await;
        let _ = timeout_at(closing_deadline, output.shutdown()).await;
        // Timeout leaves the handle retained in IoOwner. The service registry
        // must retain self/io and refuse replacement until explicit join.
        timeout_at(closing_deadline, self.io.join_pending())
            .await
            .map_err(|_| ControlError::NeedsReconciliation)??;
        result
    }

    async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &mut self,
        input: &mut R,
        output: &mut W,
    ) -> Result<(), ControlError> {
        self.current_admission().await?;
        let handshake_reservation = Arc::new(self.bytes.reserve()?);
        self.io.run(IoCommand::Check, handshake_reservation).await?;
        let mut state = ChannelState::new(self.hello.clone(), self.binding.endpoint.clone())
            .map_err(|_| ControlError::Binding)?;
        let handshake = Instant::now() + FRAME_TIME;
        timeout_at(
            handshake,
            checked_write(output, &FrameV1::Hello(self.hello.clone()), &self.secrets),
        )
        .await
        .map_err(|_| ControlError::Timeout)??;
        let ready = timeout_at(handshake, read_frame(input, FrameLimit::Control))
            .await
            .map_err(|_| ControlError::Timeout)??;
        let Some(FrameV1::Ready(ready)) = ready else {
            return Err(ControlError::Protocol);
        };
        state
            .accept_ready(&ready)
            .map_err(|_| ControlError::Binding)?;
        loop {
            state
                .request_capacity()
                .map_err(|_| ControlError::Capacity)?;
            // Reserve before even reading the prefix. No body is read without
            // capacity and the previous output having completed.
            let reservation = Arc::new(self.bytes.reserve()?);
            let request = match read_frame(input, FrameLimit::Request).await? {
                Some(FrameV1::Request(request)) => request,
                None => return Ok(()),
                _ => return Err(ControlError::Protocol),
            };
            state
                .admit_request(&request)
                .map_err(|_| ControlError::Protocol)?;
            let deadline = Instant::now() + OPERATION_TIME;
            let response = timeout_at(
                deadline,
                self.exchange(input, &request, reservation.clone()),
            )
            .await
            .map_err(|_| ControlError::Timeout)??;
            state
                .accept_response(&response)
                .map_err(|_| ControlError::Protocol)?;
            let response_frame = FrameV1::Response(response);
            let mut prefix = [0_u8; 1];
            tokio::select! {
                biased;
                activity = input.read(&mut prefix) => return Err(peer_activity(activity)),
                result = timeout_at(deadline, checked_write(output, &response_frame, &self.secrets)) => result.map_err(|_| ControlError::Timeout)??,
            }
            // The same reservation covers output backpressure. Pending IO has
            // its own Arc clone until its real completion/join, not new permits.
            drop(reservation);
        }
    }

    async fn exchange<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
        request: &RequestV1,
        reservation: Arc<Reservation>,
    ) -> Result<ResponseV1, ControlError> {
        self.current_admission().await?;
        let mutation = !matches!(&request.payload, RequestPayloadV1::Inbox(_));
        if mutation {
            if let Err(error) = self
                .io
                .run(
                    IoCommand::Prepare(request.payload.clone()),
                    reservation.clone(),
                )
                .await
            {
                let code = match error {
                    ControlError::InvalidRequest => ErrorCodeV1::InvalidRequest,
                    ControlError::Conflict => ErrorCodeV1::Conflict,
                    ControlError::Capacity => ErrorCodeV1::Capacity,
                    fatal => return Err(fatal),
                };
                return Ok(response(request, ResponseResultV1::Error(code)));
            }
            self.current_admission().await?;
            // Durable intent and dispatch marker both precede the HTTP future.
            self.io
                .run(
                    IoCommand::Dispatched(request.payload.clone()),
                    reservation.clone(),
                )
                .await?;
        }
        let mut prefix = [0_u8; 1];
        let result = tokio::select! {
            biased;
            activity = input.read(&mut prefix) => return Err(peer_activity(activity)),
            result = dispatch(&self.client, &request.payload) => result,
        };
        if let Ok(ReplyV1::Send(receipt) | ReplyV1::Ack(receipt)) = &result {
            // A receipt must be valid and durable before local success delivery.
            self.io
                .run(
                    IoCommand::Confirmed(request.payload.clone(), receipt.clone()),
                    reservation,
                )
                .await?;
        }
        // Any non-confirmed dispatch remains reserved. In particular, a current
        // Forbidden/Authentication response cannot erase a prior unknown result.
        Ok(response(
            request,
            match result {
                Ok(reply) => ResponseResultV1::Ok(reply),
                Err(failure) => match (request.version, failure.error, failure.refusal) {
                    (2, RelayClientError::Capacity, Some(refusal)) => {
                        ResponseResultV1::SendRefusal(refusal)
                    }
                    (_, error, _) => ResponseResultV1::Error(error_code(error)),
                },
            },
        ))
    }
}

fn response(request: &RequestV1, result: ResponseResultV1) -> ResponseV1 {
    ResponseV1 {
        version: request.version,
        channel_id: request.channel_id.clone(),
        epoch: request.epoch,
        correlation_id: request.correlation_id.clone(),
        operation: request.operation(),
        result,
    }
}
async fn dispatch(
    client: &WorkerRelayClient,
    payload: &RequestPayloadV1,
) -> Result<ReplyV1, crate::relay::RelaySendFailureV1> {
    match payload {
        RequestPayloadV1::Send(input) => client.send_with_refusal(input).await.map(ReplyV1::Send),
        RequestPayloadV1::Inbox(input) => client
            .inbox(input.after, input.limit)
            .await
            .map(ReplyV1::Inbox)
            .map_err(Into::into),
        RequestPayloadV1::Ack(input) => client
            .ack(input)
            .await
            .map(ReplyV1::Ack)
            .map_err(Into::into),
    }
}
fn client_error(error: RelayClientError) -> ControlError {
    match error {
        RelayClientError::InvalidRequest | RelayClientError::InvalidConfiguration => {
            ControlError::InvalidRequest
        }
        _ => ControlError::Protocol,
    }
}
fn error_code(error: RelayClientError) -> ErrorCodeV1 {
    match error {
        RelayClientError::InvalidConfiguration | RelayClientError::InvalidRequest => {
            ErrorCodeV1::InvalidRequest
        }
        RelayClientError::Authentication => ErrorCodeV1::Authentication,
        RelayClientError::Forbidden => ErrorCodeV1::Forbidden,
        RelayClientError::NotFound => ErrorCodeV1::NotFound,
        RelayClientError::Conflict => ErrorCodeV1::Conflict,
        RelayClientError::Capacity => ErrorCodeV1::Capacity,
        RelayClientError::Unavailable => ErrorCodeV1::Unavailable,
        RelayClientError::Transport => ErrorCodeV1::Transport,
        RelayClientError::Protocol => ErrorCodeV1::Protocol,
    }
}
fn peer_activity(result: std::io::Result<usize>) -> ControlError {
    match result {
        Ok(0) => ControlError::PeerClosed,
        Ok(_) => ControlError::ExtraFrame,
        Err(_) => ControlError::Transport,
    }
}

/// Cancellation discards the whole owning future and its streams. Reads are
/// never restarted on a different frame or searched for a later delimiter.
async fn read_frame<R: AsyncRead + Unpin>(
    input: &mut R,
    limit: FrameLimit,
) -> Result<Option<FrameV1>, ControlError> {
    let mut prefix = [0_u8; 4];
    let count = input
        .read(&mut prefix[..1])
        .await
        .map_err(|_| ControlError::Transport)?;
    if count == 0 {
        return Ok(None);
    }
    let absolute = Instant::now() + FRAME_TIME;
    read_exact(input, &mut prefix[1..], absolute).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    let maximum = match limit {
        FrameLimit::Control => MAX_CONTROL_FRAME_BYTES,
        FrameLimit::Request => MAX_REQUEST_FRAME_BYTES,
        FrameLimit::Response => super::MAX_RESPONSE_FRAME_BYTES,
    };
    if length == 0 || length > maximum {
        return Err(ControlError::Protocol);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| ControlError::Capacity)?;
    bytes.resize(length, 0);
    read_exact(input, &mut bytes, absolute).await?;
    FrameV1::decode(&bytes, limit)
        .map(Some)
        .map_err(|_| ControlError::Protocol)
}
async fn read_exact<R: AsyncRead + Unpin>(
    input: &mut R,
    bytes: &mut [u8],
    absolute: Instant,
) -> Result<(), ControlError> {
    let mut offset = 0;
    while offset < bytes.len() {
        let deadline = absolute.min(Instant::now() + IDLE_TIME);
        let count = timeout_at(deadline, input.read(&mut bytes[offset..]))
            .await
            .map_err(|_| ControlError::Timeout)?
            .map_err(|_| ControlError::Transport)?;
        if count == 0 {
            return Err(ControlError::Protocol);
        }
        offset += count;
    }
    Ok(())
}
async fn checked_write<W: AsyncWrite + Unpin>(
    output: &mut W,
    frame: &FrameV1,
    secrets: &Secrets,
) -> Result<(), ControlError> {
    let bytes = frame.encode().map_err(|_| ControlError::Protocol)?;
    secrets.check(&bytes)?;
    let prefix = (bytes.len() as u32).to_be_bytes();
    let absolute = Instant::now() + FRAME_TIME;
    for part in [&prefix[..], &bytes[..]] {
        let mut offset = 0;
        while offset < part.len() {
            let deadline = absolute.min(Instant::now() + IDLE_TIME);
            let count = timeout_at(deadline, output.write(&part[offset..]))
                .await
                .map_err(|_| ControlError::Timeout)?
                .map_err(|_| ControlError::Transport)?;
            if count == 0 {
                return Err(ControlError::Transport);
            }
            offset += count;
        }
    }
    timeout_at(absolute.min(Instant::now() + IDLE_TIME), output.flush())
        .await
        .map_err(|_| ControlError::Timeout)?
        .map_err(|_| ControlError::Transport)
}

#[cfg(test)]
mod tests;
