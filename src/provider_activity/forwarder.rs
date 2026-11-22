//! Deliver spooled activity to the gateway in bounded, idempotent batches.
//!
//! Delivery never removes spooled events. A batch is retried with backoff;
//! because each event id derives from its spool position, a replay after an
//! interruption returns the stored rows instead of appending duplicates.
use super::collector::ActivityCollector;
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
    pub delivered_through: u64,
    pub failed_attempts: u64,
    pub last_error: Option<String>,
    /// Events still pending when the drain deadline ended the forwarder.
    pub undelivered: u64,
}

/// Run until `stop` becomes true and the spool is drained, or until the drain
/// deadline after `stop` expires. Never drops an event; undelivered events
/// stay in the spool for a later recovery run.
pub async fn forward<S: ActivitySink>(
    collector: ActivityCollector,
    sink: &S,
    mut stop: watch::Receiver<bool>,
    drain: Duration,
) -> ForwardOutcome {
    let mut outcome = ForwardOutcome::default();
    let mut backoff = MIN_BACKOFF;
    let mut drain_until: Option<Instant> = None;
    loop {
        if drain_until.is_none() && *stop.borrow() {
            drain_until = Some(Instant::now() + drain);
        }
        let batch = collector.pending(MAX_BATCH_EVENTS, MAX_BATCH_BYTES);
        if batch.is_empty() {
            if drain_until.is_some() {
                break;
            }
            // A dropped owner is a stop: drain, never spin on a closed channel.
            if let Ok(Err(_)) = tokio::time::timeout(IDLE_POLL, stop.changed()).await {
                drain_until = Some(Instant::now() + drain);
            }
            continue;
        }
        if drain_until.is_some_and(|until| Instant::now() >= until) {
            break;
        }
        let through = batch.last().map_or(0, |event| event.seq);
        let events: Vec<Event> = batch.into_iter().map(|spooled| spooled.event).collect();
        let attempt = match drain_until {
            Some(until) => tokio::time::timeout_at(until, sink.deliver(&events))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("activity drain deadline reached"))),
            None => sink.deliver(&events).await,
        };
        match attempt {
            Ok(()) => {
                backoff = MIN_BACKOFF;
                if collector.mark_delivered(through).is_ok() {
                    outcome.delivered_through = through;
                }
            }
            Err(error) => {
                outcome.failed_attempts += 1;
                outcome.last_error = Some(crate::redaction::clip(&format!("{error:#}"), 300));
                let wait = match drain_until {
                    Some(until) => backoff.min(until.saturating_duration_since(Instant::now())),
                    None => backoff,
                };
                tokio::time::sleep(wait).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    outcome.undelivered = collector
        .summary()
        .map_or(0, |s| s.spooled_events.saturating_sub(s.delivered_events));
    outcome
}
