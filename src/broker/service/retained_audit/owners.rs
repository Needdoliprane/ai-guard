//! Bounded cold-owner tasks. Execution permits and Full runs are independent.
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

type OwnerResult = Result<RetainedAuditSuccess, BrokerErrorCodeV1>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::broker::service::server) enum ColdOwnerError {
    InvalidCapacity,
    Busy,
    Closed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::broker::service::server) enum ColdDrainError {
    OwnerFailed,
    Unavailable,
}

impl fmt::Display for ColdOwnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCapacity => "invalid cold owner capacity",
            Self::Busy => "cold owner capacity unavailable",
            Self::Closed => "cold owner registration closed",
            Self::Unavailable => "cold owner tracking unavailable",
        })
    }
}
impl std::error::Error for ColdOwnerError {}

impl fmt::Display for ColdDrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OwnerFailed => "cold audit shutdown needs reconciliation",
            Self::Unavailable => "cold audit shutdown tracking unavailable",
        })
    }
}
impl std::error::Error for ColdDrainError {}

pub(in crate::broker::service::server) struct ColdOwners {
    state: Mutex<State>,
    // Serialize observers only, so two JoinHandle polls cannot replace each
    // other's wakers. Producers never acquire this permit; it is unrelated to
    // execution capacity. Dropping a drain future releases it, not a handle.
    observers: Semaphore,
}

struct State {
    open: bool,
    slots: Vec<Slot>,
    failure: Option<ColdDrainError>,
}

enum Slot {
    Vacant,
    Reserved,
    Running(JoinHandle<OwnerResult>),
}

pub(in crate::broker::service::server) struct ColdOwnerSlot {
    owners: Arc<ColdOwners>,
    index: Option<usize>,
}

impl fmt::Debug for ColdOwners {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColdOwners(<private>)")
    }
}
impl fmt::Debug for ColdOwnerSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColdOwnerSlot(<private>)")
    }
}

impl State {
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
                    failure.get_or_insert(ColdDrainError::OwnerFailed);
                }
                *slot = Slot::Vacant;
            }
        }
    }
}

impl ColdOwners {
    pub(in crate::broker::service::server) fn new(
        capacity: usize,
    ) -> Result<Arc<Self>, ColdOwnerError> {
        if !(1..=1024).contains(&capacity) {
            return Err(ColdOwnerError::InvalidCapacity);
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

    fn state(&self) -> (MutexGuard<'_, State>, bool) {
        match self.state.lock() {
            Ok(state) => (state, false),
            Err(error) => {
                // A poisoned registry is closed/degraded, but its accepted
                // handles must still be joined instead of forgotten.
                let mut state = error.into_inner();
                state.open = false;
                state.failure.get_or_insert(ColdDrainError::Unavailable);
                (state, true)
            }
        }
    }

    pub(in crate::broker::service::server) fn try_reserve(
        self: &Arc<Self>,
    ) -> Result<ColdOwnerSlot, ColdOwnerError> {
        let (mut state, poisoned) = self.state();
        if poisoned {
            return Err(ColdOwnerError::Unavailable);
        }
        if !state.open {
            return Err(ColdOwnerError::Closed);
        }
        // Only handles already finished are polled here, never awaited. Their
        // outcome is retained even when this same call reuses their slot.
        state.reap(&mut Context::from_waker(Waker::noop()), true);
        let index = state
            .slots
            .iter()
            .position(|slot| matches!(slot, Slot::Vacant))
            .ok_or(ColdOwnerError::Busy)?;
        state.slots[index] = Slot::Reserved;
        Ok(ColdOwnerSlot {
            owners: Arc::clone(self),
            index: Some(index),
        })
    }

    pub(in crate::broker::service::server) fn close(&self) -> Result<(), ColdOwnerError> {
        let (mut state, poisoned) = self.state();
        state.open = false;
        if poisoned {
            Err(ColdOwnerError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(in crate::broker::service::server) async fn drain(&self) -> Result<(), ColdDrainError> {
        // Closing is atomic with reserve/start. Reserved tokens remain counted
        // until refusal/drop, but can no longer start or emit any HTTP. They
        // therefore do not make shutdown wait for an external token holder.
        let _ = self.close();
        let _observer = self
            .observers
            .acquire()
            .await
            .map_err(|_| ColdDrainError::Unavailable)?;
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

impl ColdOwnerSlot {
    pub(in crate::broker::service::server) fn start<F>(
        mut self,
        future: F,
    ) -> Result<(), ColdOwnerError>
    where
        F: Future<Output = OwnerResult> + Send + 'static,
    {
        let index = self.index.take().ok_or(ColdOwnerError::Unavailable)?;
        let (mut state, poisoned) = self.owners.state();
        let State {
            open,
            slots,
            failure,
        } = &mut *state;
        let Some(slot) = slots.get_mut(index) else {
            *open = false;
            failure.get_or_insert(ColdDrainError::Unavailable);
            return Err(ColdOwnerError::Unavailable);
        };
        if !matches!(slot, Slot::Reserved) {
            *open = false;
            failure.get_or_insert(ColdDrainError::Unavailable);
            return Err(ColdOwnerError::Unavailable);
        }
        if !*open || poisoned {
            *slot = Slot::Vacant;
            return Err(if poisoned {
                ColdOwnerError::Unavailable
            } else {
                ColdOwnerError::Closed
            });
        }
        // No await between closure check, spawn and handle registration. An
        // aborted producer cannot leave an accepted owner outside the tracker.
        *slot = Slot::Running(tokio::spawn(future));
        Ok(())
    }
}

impl Drop for ColdOwnerSlot {
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
