//! Bounded, typed owner tasks. Each registry has its own admission and results;
//! sharing this mechanism never converts a Cold owner into a Full capability.
//! Handles and their first failure stay here when an observer is cancelled.
use super::{BrokerErrorCodeV1, RetainedAuditSuccess};
use std::{
    fmt,
    future::{Future, poll_fn},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll, Waker},
};
use tokio::{sync::Semaphore, task::JoinHandle};

type OwnerResult<T> = Result<T, BrokerErrorCodeV1>;
pub(in crate::broker::service::server) type ColdOwners = OwnerTasks<RetainedAuditSuccess>;
pub(in crate::broker::service::server) type ColdOwnerSlot = OwnerSlot<RetainedAuditSuccess>;
pub(in crate::broker::service::server) type ColdOwnerError = OwnerError;
#[cfg(test)]
type ColdDrainError = DrainError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::broker::service::server) enum OwnerError {
    InvalidCapacity,
    Busy,
    Closed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::broker::service::server) enum DrainError {
    OwnerFailed,
    Unavailable,
}

impl fmt::Display for OwnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCapacity => "invalid broker owner capacity",
            Self::Busy => "broker owner capacity unavailable",
            Self::Closed => "broker owner registration closed",
            Self::Unavailable => "broker owner tracking unavailable",
        })
    }
}
impl std::error::Error for OwnerError {}

impl fmt::Display for DrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OwnerFailed => "broker shutdown needs reconciliation",
            Self::Unavailable => "broker shutdown tracking unavailable",
        })
    }
}
impl std::error::Error for DrainError {}

pub(in crate::broker::service::server) struct OwnerTasks<T> {
    state: Mutex<State<T>>,
    // Serialize observers only, so two JoinHandle polls cannot replace each
    // other's wakers. Producers never acquire this permit; it is unrelated to
    // execution capacity. Dropping a drain future releases it, not a handle.
    observers: Semaphore,
}

struct State<T> {
    open: bool,
    slots: Vec<Slot<T>>,
    failure: Option<DrainError>,
}

enum Slot<T> {
    Vacant,
    Reserved,
    Running(JoinHandle<OwnerResult<T>>),
}

pub(in crate::broker::service::server) struct OwnerSlot<T> {
    owners: Arc<OwnerTasks<T>>,
    index: Option<usize>,
}

impl<T> fmt::Debug for OwnerTasks<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnerTasks(<private>)")
    }
}
impl<T> fmt::Debug for OwnerSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnerSlot(<private>)")
    }
}

impl<T> State<T> {
    fn reap(&mut self, cx: &mut Context<'_>, finished_only: bool) {
        let Self { slots, failure, .. } = self;
        for slot in slots {
            let result = match slot {
                Slot::Running(handle) if !finished_only || handle.is_finished() => {
                    Pin::new(handle).poll(cx)
                }
                _ => continue,
            };
            if let Poll::Ready(result) = result {
                if !matches!(result, Ok(Ok(_))) {
                    // Record before making the slot reusable. Never format a
                    // JoinError: its panic payload can contain private data.
                    failure.get_or_insert(DrainError::OwnerFailed);
                }
                *slot = Slot::Vacant;
            }
        }
    }
}

impl<T> OwnerTasks<T> {
    pub(in crate::broker::service::server) fn new(
        capacity: usize,
    ) -> Result<Arc<Self>, OwnerError> {
        if !(1..=1024).contains(&capacity) {
            return Err(OwnerError::InvalidCapacity);
        }
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                open: true,
                slots: (0..capacity).map(|_| Slot::Vacant).collect(),
                failure: None,
            }),
            observers: Semaphore::new(1),
        }))
    }

    fn state(&self) -> (MutexGuard<'_, State<T>>, bool) {
        match self.state.lock() {
            Ok(state) => (state, false),
            Err(error) => {
                // A poisoned registry is closed/degraded, but its accepted
                // handles must still be joined instead of forgotten.
                let mut state = error.into_inner();
                state.open = false;
                state.failure.get_or_insert(DrainError::Unavailable);
                (state, true)
            }
        }
    }

    pub(in crate::broker::service::server) fn try_reserve(
        self: &Arc<Self>,
    ) -> Result<OwnerSlot<T>, OwnerError> {
        let (mut state, poisoned) = self.state();
        if poisoned {
            return Err(OwnerError::Unavailable);
        }
        if !state.open {
            return Err(OwnerError::Closed);
        }
        // Only handles already finished are polled here, never awaited. Their
        // outcome is retained even when this same call reuses their slot.
        state.reap(&mut Context::from_waker(Waker::noop()), true);
        let index = state
            .slots
            .iter()
            .position(|slot| matches!(slot, Slot::Vacant))
            .ok_or(OwnerError::Busy)?;
        state.slots[index] = Slot::Reserved;
        Ok(OwnerSlot {
            owners: Arc::clone(self),
            index: Some(index),
        })
    }

    pub(in crate::broker::service::server) fn close(&self) -> Result<(), OwnerError> {
        let (mut state, poisoned) = self.state();
        state.open = false;
        if poisoned {
            Err(OwnerError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(in crate::broker::service::server) async fn drain(&self) -> Result<(), DrainError> {
        // Closing is atomic with reserve/start. Reserved tokens remain counted
        // until refusal/drop, but can no longer start or emit any HTTP. They
        // therefore do not make shutdown wait for an external token holder.
        let _ = self.close();
        let _observer = self
            .observers
            .acquire()
            .await
            .map_err(|_| DrainError::Unavailable)?;
        poll_fn(|cx| {
            let (mut state, _) = self.state();
            state.reap(cx, false);
            if state
                .slots
                .iter()
                .any(|slot| matches!(slot, Slot::Running(_)))
            {
                Poll::Pending
            } else {
                Poll::Ready(state.failure.map_or(Ok(()), Err))
            }
            // The synchronous state lock is released before returning Pending;
            // no handle/result leaves its slot across an await or cancellation.
        })
        .await
    }
}

impl<T: Send + 'static> OwnerSlot<T> {
    pub(in crate::broker::service::server) fn start<F>(
        mut self,
        future: F,
    ) -> Result<(), OwnerError>
    where
        F: Future<Output = OwnerResult<T>> + Send + 'static,
    {
        let index = self.index.take().ok_or(OwnerError::Unavailable)?;
        let (mut state, poisoned) = self.owners.state();
        let State {
            open,
            slots,
            failure,
        } = &mut *state;
        let Some(slot) = slots.get_mut(index) else {
            *open = false;
            failure.get_or_insert(DrainError::Unavailable);
            return Err(OwnerError::Unavailable);
        };
        if !matches!(slot, Slot::Reserved) {
            *open = false;
            failure.get_or_insert(DrainError::Unavailable);
            return Err(OwnerError::Unavailable);
        }
        if !*open || poisoned {
            *slot = Slot::Vacant;
            return Err(if poisoned {
                OwnerError::Unavailable
            } else {
                OwnerError::Closed
            });
        }
        // No await between closure check, spawn and handle registration. An
        // aborted producer cannot leave an accepted owner outside the tracker.
        *slot = Slot::Running(tokio::spawn(future));
        Ok(())
    }
}

impl<T> Drop for OwnerSlot<T> {
    fn drop(&mut self) {
        let Some(index) = self.index.take() else {
            return;
        };
        let (mut state, _) = self.owners.state();
        if let Some(slot) = state.slots.get_mut(index)
            && matches!(slot, Slot::Reserved)
        {
            *slot = Slot::Vacant;
        }
    }
}

#[cfg(test)]
#[path = "owners_tests.rs"]
mod tests;
