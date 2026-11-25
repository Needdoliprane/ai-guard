//! Private queue admission for the single journal writer.
//! Queue permits are not journal, HTTP, execution, or durable budget proofs.
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueueConfigError {
    InvalidCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReserveError {
    Full,
    Disconnected,
}

pub(super) struct QueueSender<T> {
    sender: mpsc::SyncSender<Queued<T>>,
    shared: Arc<Shared>,
}

pub(super) struct QueueReceiver<T> {
    receiver: mpsc::Receiver<Queued<T>>,
    shared: Arc<Shared>,
}

/// An owned, single-use queue completion reservation, never a durable ACK.
pub(super) struct CompletionPermit<T> {
    sender: QueueSender<T>,
    admission: Admission,
}

struct Shared {
    ordinary_capacity: usize,
    ordinary: AtomicUsize,
    completion_reserved: AtomicBool,
    receiver_alive: AtomicBool,
}

enum Slot {
    Ordinary,
    Completion,
}

/// Non-Clone: ownership moves from producer to envelope, then ends at dequeue.
struct Admission {
    shared: Arc<Shared>,
    slot: Slot,
}

struct Queued<T> {
    value: T,
    admission: Admission,
}

pub(super) fn bounded<T>(
    ordinary_capacity: usize,
) -> Result<(QueueSender<T>, QueueReceiver<T>), QueueConfigError> {
    if !(1..=256).contains(&ordinary_capacity) {
        return Err(QueueConfigError::InvalidCapacity);
    }
    let physical_capacity = ordinary_capacity
        .checked_add(1)
        .ok_or(QueueConfigError::InvalidCapacity)?;
    let shared = Arc::new(Shared {
        ordinary_capacity,
        ordinary: AtomicUsize::new(0),
        completion_reserved: AtomicBool::new(false),
        receiver_alive: AtomicBool::new(true),
    });
    let (sender, receiver) = mpsc::sync_channel(physical_capacity);
    Ok((
        QueueSender {
            sender,
            shared: Arc::clone(&shared),
        },
        QueueReceiver { receiver, shared },
    ))
}

impl<T> Clone for QueueSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> QueueSender<T> {
    pub(super) fn try_send(&self, value: T) -> Result<(), mpsc::TrySendError<T>> {
        if !self.shared.receiver_alive.load(Ordering::Acquire) {
            return Err(mpsc::TrySendError::Disconnected(value));
        }
        if self
            .shared
            .ordinary
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.shared.ordinary_capacity).then(|| used + 1)
            })
            .is_err()
        {
            return Err(if self.shared.receiver_alive.load(Ordering::Acquire) {
                mpsc::TrySendError::Full(value)
            } else {
                mpsc::TrySendError::Disconnected(value)
            });
        }
        let admission = Admission {
            shared: Arc::clone(&self.shared),
            slot: Slot::Ordinary,
        };
        self.enqueue(value, admission)
    }

    pub(super) fn try_reserve_completion(&self) -> Result<CompletionPermit<T>, ReserveError> {
        if !self.shared.receiver_alive.load(Ordering::Acquire) {
            return Err(ReserveError::Disconnected);
        }
        if self
            .shared
            .completion_reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(if self.shared.receiver_alive.load(Ordering::Acquire) {
                ReserveError::Full
            } else {
                ReserveError::Disconnected
            });
        }
        let admission = Admission {
            shared: Arc::clone(&self.shared),
            slot: Slot::Completion,
        };
        if !self.shared.receiver_alive.load(Ordering::Acquire) {
            // The owned admission is released on this error, not leaked after
            // the receiver disappeared between the live check and reservation.
            return Err(ReserveError::Disconnected);
        }
        Ok(CompletionPermit {
            sender: self.clone(),
            admission,
        })
    }

    fn enqueue(&self, value: T, admission: Admission) -> Result<(), mpsc::TrySendError<T>> {
        let queued = Queued { value, admission };
        if !self.shared.receiver_alive.load(Ordering::Acquire) {
            return Err(mpsc::TrySendError::Disconnected(queued.into_value()));
        }
        // All raw sends are here. C ordinary admissions plus at most one
        // completion admission fit the physical C+1 channel. A defensive Full
        // still returns ownership and releases its admission without a panic.
        match self.sender.try_send(queued) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(queued)) => {
                Err(mpsc::TrySendError::Full(queued.into_value()))
            }
            Err(mpsc::TrySendError::Disconnected(queued)) => {
                Err(mpsc::TrySendError::Disconnected(queued.into_value()))
            }
        }
    }
}

impl<T> CompletionPermit<T> {
    pub(super) fn try_send(self, value: T) -> Result<(), mpsc::TrySendError<T>> {
        let Self { sender, admission } = self;
        // No reacquisition: this consumes the previously reserved single slot.
        sender.enqueue(value, admission)
    }
}

impl<T> QueueReceiver<T> {
    pub(super) fn recv(&self) -> Result<T, mpsc::RecvError> {
        self.receiver.recv().map(Queued::into_value)
    }

    // Closed non-blocking queue receive; production wiring pending.
    #[allow(dead_code)]
    pub(super) fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.try_recv().map(Queued::into_value)
    }
}

impl<T> Drop for QueueReceiver<T> {
    fn drop(&mut self) {
        // Mark closed before the raw receiver drops its queued envelopes. Each
        // envelope then releases its own admission even if no dequeue occurred.
        self.shared.receiver_alive.store(false, Ordering::Release);
    }
}

impl<T> Queued<T> {
    fn into_value(self) -> T {
        let Self { value, admission } = self;
        // Release queue capacity before any caller processes or retains T.
        drop(admission);
        value
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        match self.slot {
            Slot::Ordinary => {
                // Only a successful increment creates this non-Clone owner.
                self.shared.ordinary.fetch_sub(1, Ordering::AcqRel);
            }
            Slot::Completion => {
                self.shared
                    .completion_reserved
                    .store(false, Ordering::Release);
            }
        }
    }
}

impl<T> fmt::Debug for QueueSender<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QueueSender(<private>)")
    }
}

impl<T> fmt::Debug for QueueReceiver<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QueueReceiver(<private>)")
    }
}

impl<T> fmt::Debug for CompletionPermit<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CompletionPermit(<private>)")
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
