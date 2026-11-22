//! Broker side of one protocol v3 session after the attach bracket (G11, K4).
//!
//! One call drives the whole order of `session_protocol` for the session
//! mode: Handoff and archive, `imported`, `start`, `started`, the business
//! stdin and its close, the provider output, an optional broker Stop and the
//! declared `exited`. The caller has already taken the egress stream of the
//! channel (G05 owner or nothing): egress bytes never pass through here.
//!
//! Everything the worker sends is **declared**. The report keeps the
//! declared end apart from the broker's own windows and from a lost stream;
//! the observed container end is read from Kubernetes by the driver, never
//! from this report. Output bytes go to the caller's sink as they arrive and
//! only their counts are kept: no output is buffered or logged here.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "session run without its CLI entry until the session run command lands (K4)"
    )
)]
use super::session_protocol::{
    BrokerControlV3, ExitedV3, HandoffV3, ImportedV3, RefusalV3, SessionChannelV3, SessionEventsV3,
    SessionProtocolError, SessionSenderV3, StopReasonV3, WorkerControlV3, WorkerEventV3,
};
use std::time::Duration;
use tokio::{sync::watch, time::Instant};

/// Where the provider output goes. Called in arrival order; the run never
/// keeps the bytes.
pub(crate) trait SessionOutputV2 {
    fn stdout(&mut self, bytes: &[u8]);
    fn stderr(&mut self, bytes: &[u8]);
}

/// Broker windows for the declared steps before the provider runs. The
/// session deadline itself is the enforcer's armed deadline and the attach
/// relay's `session_until`, not a value of this run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionRunWaitsV2 {
    pub(crate) import: Duration,
    pub(crate) start: Duration,
    /// After `exited`, the wait for the worker to end its stream.
    pub(crate) end: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRunPhaseV2 {
    Handoff,
    Import,
    Start,
    Running,
    Ending,
}

/// How the run ended, by provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRunEndV2 {
    /// Declared by the worker: the provider ended (`exited`).
    Exited(ExitedV3),
    /// Declared by the worker: a closed refusal before the provider ran.
    Refused(RefusalV3),
    /// The broker's own window for this phase elapsed.
    Deadline(SessionRunPhaseV2),
    /// The stream failed or ended without the expected message. `protocol`
    /// distinguishes a refused frame from an I/O end.
    Lost {
        phase: SessionRunPhaseV2,
        protocol: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRunReportV2 {
    /// The declared import result, checked by the channel against the
    /// profile commitment.
    pub(crate) imported: Option<ImportedV3>,
    pub(crate) started: bool,
    pub(crate) stop_sent: Option<StopReasonV3>,
    pub(crate) end: SessionRunEndV2,
    /// After `exited`, the worker also ended its stream cleanly.
    pub(crate) stream_ended: bool,
    pub(crate) stdout_bytes: u64,
    pub(crate) stderr_bytes: u64,
}

struct Run<'a> {
    sender: SessionSenderV3,
    events: SessionEventsV3,
    output: &'a mut dyn SessionOutputV2,
    stop: &'a mut watch::Receiver<Option<StopReasonV3>>,
    report: SessionRunReportV2,
}

fn lost(phase: SessionRunPhaseV2, error: SessionProtocolError) -> SessionRunEndV2 {
    SessionRunEndV2::Lost {
        phase,
        protocol: matches!(
            error,
            SessionProtocolError::Protocol
                | SessionProtocolError::Mismatch
                | SessionProtocolError::FrameSize
        ),
    }
}

/// Runs one session to its end. `stdin` is the business standard input,
/// closed after it is sent (non-interactive transports only). `stop`
/// carries a broker Stop request; it is sent at most once.
pub(crate) async fn run_session(
    mut channel: SessionChannelV3,
    handoff: HandoffV3,
    archive: &[u8],
    stdin: &[u8],
    output: &mut dyn SessionOutputV2,
    stop: &mut watch::Receiver<Option<StopReasonV3>>,
    waits: SessionRunWaitsV2,
) -> SessionRunReportV2 {
    // The egress stream belongs to its owner; an untaken one is closed here
    // so that no egress byte waits on this run.
    drop(channel.take_egress());
    let (sender, events, _) = channel.split();
    let mut run = Run {
        sender,
        events,
        output,
        stop,
        report: SessionRunReportV2 {
            imported: None,
            started: false,
            stop_sent: None,
            end: SessionRunEndV2::Lost {
                phase: SessionRunPhaseV2::Handoff,
                protocol: false,
            },
            stream_ended: false,
            stdout_bytes: 0,
            stderr_bytes: 0,
        },
    };
    let end = run.drive(handoff, archive, stdin, waits).await;
    run.report.end = end;
    run.sender.close().await;
    run.report
}

impl Run<'_> {
    async fn drive(
        &mut self,
        handoff: HandoffV3,
        archive: &[u8],
        stdin: &[u8],
        waits: SessionRunWaitsV2,
    ) -> SessionRunEndV2 {
        use SessionRunPhaseV2::*;
        if let Err(error) = self
            .sender
            .send_control(BrokerControlV3::Handoff(Box::new(handoff)))
            .await
        {
            return lost(Handoff, error);
        }
        if let Err(error) = self.sender.send_archive(archive).await {
            return lost(Handoff, error);
        }
        match self.control(Import, Instant::now() + waits.import).await {
            Ok(WorkerControlV3::Imported(imported)) => self.report.imported = Some(imported),
            Ok(WorkerControlV3::Refused(reason)) => return SessionRunEndV2::Refused(reason),
            Ok(_) => {
                return SessionRunEndV2::Lost {
                    phase: Import,
                    protocol: true,
                };
            }
            Err(end) => return end,
        }
        // A Stop requested before Start is answered by a `stopped` refusal.
        if let Some(reason) = *self.stop.borrow_and_update() {
            return self.stop_before_start(reason, waits).await;
        }
        if let Err(error) = self.sender.send_control(BrokerControlV3::Start).await {
            return lost(Start, error);
        }
        match self.control(Start, Instant::now() + waits.start).await {
            Ok(WorkerControlV3::Started) => self.report.started = true,
            Ok(WorkerControlV3::Refused(reason)) => return SessionRunEndV2::Refused(reason),
            Ok(_) => {
                return SessionRunEndV2::Lost {
                    phase: Start,
                    protocol: true,
                };
            }
            Err(end) => return end,
        }
        if !stdin.is_empty()
            && let Err(error) = self.sender.send_stdin(stdin).await
        {
            return lost(Running, error);
        }
        if let Err(error) = self.sender.send_control(BrokerControlV3::StdinClose).await {
            return lost(Running, error);
        }
        let exited = match self.running().await {
            Ok(exited) => exited,
            Err(end) => return end,
        };
        self.report.stream_ended = self.until_end(Instant::now() + waits.end).await;
        SessionRunEndV2::Exited(exited)
    }

    async fn stop_before_start(
        &mut self,
        reason: StopReasonV3,
        waits: SessionRunWaitsV2,
    ) -> SessionRunEndV2 {
        if let Err(error) = self
            .sender
            .send_control(BrokerControlV3::Stop(reason))
            .await
        {
            return lost(SessionRunPhaseV2::Start, error);
        }
        self.report.stop_sent = Some(reason);
        match self
            .control(SessionRunPhaseV2::Start, Instant::now() + waits.start)
            .await
        {
            Ok(WorkerControlV3::Refused(refusal)) => SessionRunEndV2::Refused(refusal),
            Ok(_) => SessionRunEndV2::Lost {
                phase: SessionRunPhaseV2::Start,
                protocol: true,
            },
            Err(end) => end,
        }
    }

    /// The next control message before `until`. Output before it is a
    /// protocol refusal, never forwarded.
    async fn control(
        &mut self,
        phase: SessionRunPhaseV2,
        until: Instant,
    ) -> Result<WorkerControlV3, SessionRunEndV2> {
        match tokio::time::timeout_at(until, self.events.next()).await {
            Err(_) => Err(SessionRunEndV2::Deadline(phase)),
            Ok(Err(error)) => Err(lost(phase, error)),
            Ok(Ok(None)) => Err(SessionRunEndV2::Lost {
                phase,
                protocol: false,
            }),
            Ok(Ok(Some(WorkerEventV3::Control(message)))) => Ok(message),
            Ok(Ok(Some(_))) => Err(SessionRunEndV2::Lost {
                phase,
                protocol: true,
            }),
        }
    }

    /// Output until `exited`. A Stop request is sent at most once, and it is
    /// polled before the output, so that a flood of output never delays it;
    /// once its requester is gone, output is still read until `exited`.
    async fn running(&mut self) -> Result<ExitedV3, SessionRunEndV2> {
        let mut requester = true;
        loop {
            let listen = requester && self.report.stop_sent.is_none();
            tokio::select! {
                biased;
                changed = self.stop.changed(), if listen => match changed {
                    Err(_) => requester = false,
                    Ok(()) => {
                        let requested = *self.stop.borrow_and_update();
                        if let Some(reason) = requested {
                            if let Err(error) =
                                self.sender.send_control(BrokerControlV3::Stop(reason)).await
                            {
                                return Err(lost(SessionRunPhaseV2::Running, error));
                            }
                            self.report.stop_sent = Some(reason);
                        }
                    }
                },
                event = self.events.next() => {
                    if let Some(end) = self.output_event(event) {
                        return end;
                    }
                }
            }
        }
    }

    /// One event while the provider runs: `None` to continue.
    fn output_event(
        &mut self,
        event: Result<Option<WorkerEventV3>, SessionProtocolError>,
    ) -> Option<Result<ExitedV3, SessionRunEndV2>> {
        use SessionRunPhaseV2::Running;
        match event {
            Err(error) => Some(Err(lost(Running, error))),
            Ok(None) => Some(Err(SessionRunEndV2::Lost {
                phase: Running,
                protocol: false,
            })),
            Ok(Some(WorkerEventV3::Stdout(bytes))) => {
                self.report.stdout_bytes += bytes.len() as u64;
                self.output.stdout(&bytes);
                None
            }
            Ok(Some(WorkerEventV3::Stderr(bytes))) => {
                self.report.stderr_bytes += bytes.len() as u64;
                self.output.stderr(&bytes);
                None
            }
            Ok(Some(WorkerEventV3::Control(WorkerControlV3::Exited(exited)))) => Some(Ok(exited)),
            Ok(Some(_)) => Some(Err(SessionRunEndV2::Lost {
                phase: Running,
                protocol: true,
            })),
        }
    }

    /// `true` when the worker ends its stream cleanly before `until`.
    async fn until_end(&mut self, until: Instant) -> bool {
        matches!(
            tokio::time::timeout_at(until, self.events.next()).await,
            Ok(Ok(None))
        )
    }
}

#[cfg(test)]
#[path = "session_run_tests.rs"]
mod tests;
