//! Bounded scheduling shared by the owned actors. Admitted filesystem work is
//! always joined, the common stop has priority, and one retained operation is
//! retried only according to its closed retry class at a bounded cadence.
use super::*;
use crate::broker::presence::actor;
use crate::broker_registry::owned_supervision::OwnedRetryClassV2;
use tokio::sync::OwnedSemaphorePermit;

/// The legacy short burst, then the slow cadence, for one retained operation.
const SHORT_BACKOFF_MILLIS: [u64; 3] = [100, 250, 500];
const SLOW_RETRY_SECONDS: u32 = 30;
/// Slow, bounded cadence while the control operator must act.
const OPERATOR_WAIT_SECONDS: u64 = 60;

pub(super) async fn disk<T: Send + 'static>(
    p: &Arc<PreparedOwnedPresence>,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
    action: impl FnOnce(&PreparedOwnedPresence) -> Result<T> + Send + 'static,
) -> Result<T> {
    let capacity = actor::interrupt(shutdown, async {
        Arc::clone(io)
            .acquire_owned()
            .await
            .map_err(|_| PresenceError::Unavailable)
    })
    .await?;
    let p = Arc::clone(p);
    // Admission can stop; an admitted worker is always joined. It holds the
    // retained profile and capacity and never contains an HTTP client.
    let task = tokio::task::spawn_blocking(move || {
        let _capacity = capacity;
        p.revalidate()?;
        action(&p)
    });
    let result = task.await.map_err(|error| {
        if error.is_cancelled() {
            // The runtime shut down before this admitted action began.
            PresenceError::Stopped
        } else {
            // A panicking journal action leaves its effect unknown: stop with
            // the Pending retained, never retry the same panic.
            PresenceError::UnsafeState
        }
    })?;
    // The common stop has priority over continuation after the joined FS
    // result: no self-read or POST may begin on this stopped branch.
    match result {
        Err(error) => Err(error),
        Ok(_) if *shutdown.borrow() || shutdown.has_changed().is_err() => {
            Err(PresenceError::Stopped)
        }
        Ok(value) => Ok(value),
    }
}

/// One bounded in-flight attempt; the permit is shared with every profile.
pub(super) async fn permit(
    shutdown: &mut watch::Receiver<bool>,
    attempts: &Arc<Semaphore>,
) -> Result<OwnedSemaphorePermit> {
    actor::interrupt(shutdown, async {
        Arc::clone(attempts)
            .acquire_owned()
            .await
            .map_err(|_| PresenceError::Unavailable)
    })
    .await
}

pub(super) async fn pause(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> Result<()> {
    actor::interrupt(shutdown, async {
        tokio::time::sleep(delay).await;
        Ok(())
    })
    .await
}

/// Only a classified owned state, a lost exchange or a local admission
/// shortage is retried, and always with the same retained operation. Any
/// other outcome, including an unrecognized named 503 or a failed journal
/// action (mapped to its own closed error by `disk`), stops the actor.
fn class(error: PresenceError) -> OwnedRetryClassV2 {
    match error {
        // The request may have been committed: reconcile by exact self-read.
        PresenceError::Transport => OwnedRetryClassV2::ReconcileSameOperation,
        PresenceError::Unavailable => OwnedRetryClassV2::RetrySameOperation,
        PresenceError::Owned(OwnedPresenceBlock::Remote(failure)) => failure.retry(),
        _ => OwnedRetryClassV2::Stop,
    }
}

/// Retry state of one retained operation, with its named slow states.
pub(super) struct Retry {
    journal: &'static str,
    short: usize,
    /// The named slow state last reported for this operation, if any.
    reported: Option<&'static str>,
}
impl Retry {
    pub(super) fn new(journal: &'static str) -> Self {
        Self {
            journal,
            short: 0,
            reported: None,
        }
    }

    /// Wait before the same operation is attempted again, or return the error
    /// that stops the actor with its Pending retained.
    pub(super) async fn after(
        &mut self,
        p: &PreparedOwnedPresence,
        shutdown: &mut watch::Receiver<bool>,
        error: PresenceError,
    ) -> Result<()> {
        let retry = class(error);
        let delay = match retry {
            OwnedRetryClassV2::Stop => return Err(error),
            OwnedRetryClassV2::RetrySameOperation | OwnedRetryClassV2::ReconcileSameOperation => {
                if let Some(millis) = SHORT_BACKOFF_MILLIS.get(self.short) {
                    self.short += 1;
                    Duration::from_millis(*millis)
                } else {
                    // A long transport cut or unavailability stays visible.
                    self.enter(p, "publication_retrying", error, retry);
                    Duration::from_secs(u64::from(
                        p.config.interval_seconds.min(SLOW_RETRY_SECONDS),
                    ))
                }
            }
            OwnedRetryClassV2::WaitForOperator => {
                self.enter(p, "publication_waiting_for_operator", error, retry);
                Duration::from_secs(u64::from(p.config.interval_seconds).max(OPERATOR_WAIT_SECONDS))
            }
        };
        pause(shutdown, delay).await
    }

    /// One closed event per transition between named slow states.
    fn enter(
        &mut self,
        p: &PreparedOwnedPresence,
        event: &'static str,
        error: PresenceError,
        retry: OwnedRetryClassV2,
    ) {
        if self.reported != Some(event) {
            self.reported = Some(event);
            p.report(self.journal, event, Some(error), Some(retry.retry_action()));
        }
    }

    /// The retained operation was acknowledged durably.
    pub(super) fn resolved(&mut self, p: &PreparedOwnedPresence) {
        if self.reported.take().is_some() {
            p.report(self.journal, "publication_resumed", None, None);
        }
    }
}
