//! Deliver spooled activity to the gateway in bounded, idempotent batches.
//!
//! Delivery never removes spooled events. A batch is retried with backoff;
//! because each event id derives from its spool position, a replay after an
//! interruption returns the stored rows instead of appending duplicates.
use super::{collector::ActivityCollector, spool::SpooledEvent};
use crate::model::Event;
use std::time::Duration;
use tokio::{sync::watch, time::Instant};

/// Gateway batch bounds (`POST /api/v1/events/batch`).
pub const MAX_BATCH_EVENTS: usize = 64;
pub const MAX_BATCH_BYTES: usize = 200 * 1024;
const IDLE_POLL: Duration = Duration::from_millis(200);
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Where delivered activity goes. Production: the gateway batch route.
pub trait ActivitySink: Send + Sync {
    fn deliver(&self, events: &[Event]) -> impl Future<Output = anyhow::Result<()>> + Send;
}

impl ActivitySink for crate::runner::AuditClient {
    fn deliver(&self, events: &[Event]) -> impl Future<Output = anyhow::Result<()>> + Send {
        self.post_events_batch(events)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForwardOutcome {
    /// Last delivery marker verified in this invocation.
    pub delivered_through: u64,
    pub failed_attempts: u64,
    pub last_error: Option<String>,
    /// Events without a local delivery marker at shutdown. An unavailable
    /// cursor or an unconfirmed marker leaves this count unknown.
    pub undelivered: Option<u64>,
}

struct DeliveryCursor {
    spooled: u64,
    delivered: u64,
}

trait ForwardSpool: Sync {
    fn batch(&self) -> Vec<SpooledEvent>;
    fn persist_delivery(&self, through: u64) -> anyhow::Result<()>;
    fn delivery_cursor(&self) -> anyhow::Result<DeliveryCursor>;
}

impl ForwardSpool for ActivityCollector {
    fn batch(&self) -> Vec<SpooledEvent> {
        self.pending(MAX_BATCH_EVENTS, MAX_BATCH_BYTES)
    }

    fn persist_delivery(&self, through: u64) -> anyhow::Result<()> {
        self.mark_delivered(through)
    }

    fn delivery_cursor(&self) -> anyhow::Result<DeliveryCursor> {
        let summary = self.summary()?;
        Ok(DeliveryCursor {
            spooled: summary.spooled_events,
            delivered: summary.delivered_events,
        })
    }
}

/// Forward until the spool drains after stop, or a delivery confirmation
/// becomes unavailable. Stop starts a deadline for asynchronous waits.
/// The spool retains its events for a later recovery run.
pub async fn forward<S: ActivitySink>(
    collector: ActivityCollector,
    sink: &S,
    stop: watch::Receiver<bool>,
    drain: Duration,
) -> ForwardOutcome {
    forward_spool(&collector, sink, stop, drain).await
}

async fn forward_spool(
    spool: &impl ForwardSpool,
    sink: &impl ActivitySink,
    mut stop: watch::Receiver<bool>,
    drain: Duration,
) -> ForwardOutcome {
    let mut outcome = ForwardOutcome::default();
    let mut backoff = MIN_BACKOFF;
    let mut drain_until: Option<Instant> = None;
    loop {
        if drain_until.is_none() && (*stop.borrow() || stop.has_changed().is_err()) {
            drain_until = Some(Instant::now() + drain);
        }
        if drain_until.is_some_and(|until| Instant::now() >= until) {
            break;
        }
        let batch = spool.batch();
        if batch.is_empty() {
            if drain_until.is_some() {
                break;
            }
            // A dropped owner is a stop: drain, never spin on a closed channel.
            if tokio::time::timeout(IDLE_POLL, wait_for_stop(&mut stop))
                .await
                .is_ok()
            {
                drain_until = Some(Instant::now() + drain);
            }
            continue;
        }
        let through = batch.last().map_or(0, |event| event.seq);
        let events: Vec<Event> = batch.into_iter().map(|spooled| spooled.event).collect();
        let attempt = wait_with_drain(
            async { sink.deliver(&events).await },
            &mut stop,
            &mut drain_until,
            drain,
        )
        .await
        .unwrap_or_else(|| Err(anyhow::anyhow!("activity drain deadline reached")));
        match attempt {
            Ok(()) => {
                let marker = spool.persist_delivery(through).and_then(|()| {
                    let cursor = spool.delivery_cursor()?;
                    anyhow::ensure!(
                        cursor.delivered >= through && cursor.delivered <= cursor.spooled,
                        "provider activity delivery marker did not advance"
                    );
                    Ok(())
                });
                if let Err(error) = marker {
                    outcome.failed_attempts += 1;
                    outcome.last_error = Some(crate::redaction::clip(&format!("{error:#}"), 300));
                    // The gateway already acknowledged this batch. Leave the
                    // spool available for replay from a reopened cursor.
                    return outcome;
                }
                outcome.delivered_through = through;
                backoff = MIN_BACKOFF;
            }
            Err(error) => {
                outcome.failed_attempts += 1;
                outcome.last_error = Some(crate::redaction::clip(&format!("{error:#}"), 300));
                if wait_with_drain(
                    tokio::time::sleep(backoff),
                    &mut stop,
                    &mut drain_until,
                    drain,
                )
                .await
                .is_none()
                {
                    break;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    match spool.delivery_cursor().and_then(|cursor| {
        cursor
            .spooled
            .checked_sub(cursor.delivered)
            .ok_or_else(|| anyhow::anyhow!("invalid provider activity delivery cursor"))
    }) {
        Ok(count) => outcome.undelivered = Some(count),
        Err(error) => {
            outcome.last_error = Some(crate::redaction::clip(&format!("{error:#}"), 300));
        }
    }
    outcome
}

async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Start the drain budget while keeping the same request or backoff in flight.
async fn wait_with_drain<T>(
    work: impl Future<Output = T>,
    stop: &mut watch::Receiver<bool>,
    drain_until: &mut Option<Instant>,
    drain: Duration,
) -> Option<T> {
    let mut work = std::pin::pin!(work);
    let until = match *drain_until {
        Some(until) => until,
        None => tokio::select! {
            biased;
            _ = wait_for_stop(stop) => *drain_until.insert(Instant::now() + drain),
            result = &mut work => return Some(result),
        },
    };
    if Instant::now() >= until {
        return None;
    }
    let result = tokio::time::timeout_at(until, &mut work).await.ok()?;
    (Instant::now() < until).then_some(result)
}

#[cfg(test)]
#[path = "forwarder_tests.rs"]
mod tests;
