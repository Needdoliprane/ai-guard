//! One retained owner per consumed profile. Cancelling a join observer cannot
//! remove its handle; a common stop drains both actors and admitted FS work.
use super::*;
use crate::broker_registry::BrokerCheckpointSnapshotV1;
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe};
use tokio::{sync::Mutex, task::JoinHandle};

struct State {
    task: Option<JoinHandle<Result<()>>>,
    result: Option<Result<()>>,
}
pub(crate) struct PresenceRunOwner {
    stop: watch::Sender<bool>,
    state: Mutex<State>,
}
impl PresenceRunOwner {
    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
    }
    /// Cancellation drops only this observer's mutex guard. The owned handle
    /// stays in State for the service's next join, including failed results.
    pub(crate) async fn join(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if let Some(result) = state.result {
            return result;
        }
        let result = state
            .task
            .as_mut()
            .ok_or(PresenceError::Unavailable)?
            .await
            .unwrap_or(Err(PresenceError::Unavailable));
        state.task.take();
        state.result = Some(result);
        result
    }
    #[cfg(test)]
    pub(super) async fn stopped(&self) {
        let mut stopped = self.stop.subscribe();
        actor::stopping(&mut stopped).await;
    }
}
impl Drop for PresenceRunOwner {
    fn drop(&mut self) {
        // Dropping the owner requests stop but is never a successful join.
        // The supervisor still owns and drains its two actor futures. Services
        // retain this owner until join rather than relying on this fallback.
        self.stop();
    }
}

pub(super) fn start(
    p: PreparedPresence,
    snapshots: Option<watch::Receiver<Option<BrokerCheckpointSnapshotV1>>>,
    mut external: watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<PresenceRunOwner> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PresenceError::Unavailable)?;
    let (stop, stopped) = watch::channel(false);
    let signal = stop.clone();
    let task = runtime.spawn(async move {
        let p = Arc::new(p);
        let mut heartbeat_stop = stopped.clone();
        let mut checkpoint_stop = stopped;
        let heartbeat_signal = signal.clone();
        let checkpoint_signal = signal.clone();
        let heartbeat_p = Arc::clone(&p);
        let heartbeat_attempts = Arc::clone(&attempts);
        let heartbeat_io = Arc::clone(&io);
        let heartbeat = async move {
            let result = AssertUnwindSafe(actor::run_owned(
                heartbeat_p,
                &mut heartbeat_stop,
                heartbeat_attempts,
                heartbeat_io,
            ))
            .catch_unwind()
            .await
            .unwrap_or(Err(PresenceError::Unavailable));
            heartbeat_signal.send_replace(true);
            result
        };
        let checkpoint = async move {
            if let Some(snapshots) = snapshots {
                let result = AssertUnwindSafe(checkpoint_publisher::run(
                    p,
                    snapshots,
                    &mut checkpoint_stop,
                    attempts,
                    io,
                ))
                .catch_unwind()
                .await
                .unwrap_or(Err(PresenceError::Unavailable));
                checkpoint_signal.send_replace(true);
                result
            } else {
                // Legacy V1 has only its existing heartbeat actor. Do not
                // terminate it merely because the checkpoint branch is absent.
                Ok(())
            }
        };
        let joined = async { tokio::join!(heartbeat, checkpoint) };
        tokio::pin!(joined);
        let results = tokio::select! { biased;
            _ = actor::stopping(&mut external) => {
                signal.send_replace(true);
                joined.await
            },
            result = &mut joined => result,
        };
        for result in [results.0, results.1] {
            if let Err(error) = result
                && error != PresenceError::Stopped
            {
                return Err(error);
            }
        }
        Ok(())
    });
    Ok(PresenceRunOwner {
        stop,
        state: Mutex::new(State {
            task: Some(task),
            result: None,
        }),
    })
}

/// Supervision of another profile's actors with the same retained join, stop
/// priority and unwinding rules as [`start`]. Each actor receives the common
/// stop; the first to finish stops the other. An absent checkpoint actor never
/// terminates the heartbeat actor, exactly like the legacy V1 profile.
pub(super) fn supervise<HF, H, CF, C>(
    mut external: watch::Receiver<bool>,
    heartbeat: HF,
    checkpoint: Option<CF>,
) -> Result<PresenceRunOwner>
where
    HF: FnOnce(watch::Receiver<bool>) -> H + Send + 'static,
    H: Future<Output = Result<()>> + Send + 'static,
    CF: FnOnce(watch::Receiver<bool>) -> C + Send + 'static,
    C: Future<Output = Result<()>> + Send + 'static,
{
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PresenceError::Unavailable)?;
    let (stop, stopped) = watch::channel(false);
    let signal = stop.clone();
    let task = runtime.spawn(async move {
        let heartbeat_signal = signal.clone();
        let checkpoint_signal = signal.clone();
        let heartbeat_actor = heartbeat(stopped.clone());
        let checkpoint_actor = checkpoint.map(|start| start(stopped));
        let heartbeat = async move {
            let result = AssertUnwindSafe(heartbeat_actor)
                .catch_unwind()
                .await
                .unwrap_or(Err(PresenceError::Unavailable));
            heartbeat_signal.send_replace(true);
            result
        };
        let checkpoint = async move {
            let Some(checkpoint_actor) = checkpoint_actor else {
                return Ok(());
            };
            let result = AssertUnwindSafe(checkpoint_actor)
                .catch_unwind()
                .await
                .unwrap_or(Err(PresenceError::Unavailable));
            checkpoint_signal.send_replace(true);
            result
        };
        let joined = async { tokio::join!(heartbeat, checkpoint) };
        tokio::pin!(joined);
        let results = tokio::select! { biased;
            _ = actor::stopping(&mut external) => {
                signal.send_replace(true);
                joined.await
            },
            result = &mut joined => result,
        };
        for result in [results.0, results.1] {
            if let Err(error) = result
                && error != PresenceError::Stopped
            {
                return Err(error);
            }
        }
        Ok(())
    });
    Ok(PresenceRunOwner {
        stop,
        state: Mutex::new(State {
            task: Some(task),
            result: None,
        }),
    })
}
