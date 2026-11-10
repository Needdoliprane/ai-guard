//! Fixed, nonblocking SSE admission. No Store permit or I/O belongs to a lease.
use crate::identity::PrincipalRefV1;
use futures_util::Stream;
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) const TOTAL_STREAM_LIMIT: usize = 32;
const HUMAN_STREAM_LIMIT: usize = 24;
const PRINCIPAL_STREAM_LIMIT: usize = 4;

#[derive(Debug)]
pub(super) enum AdmissionFailure {
    Capacity,
    Unavailable,
}

pub(super) struct Admission {
    humans: Arc<Semaphore>,
    // Each tenant owns its own registry. Only canonical PrincipalRefV1 keys
    // enter it; credentials, contexts and memberships never create a new share.
    principals: Mutex<HashMap<String, Weak<Semaphore>>>,
}

impl Admission {
    pub(super) fn new() -> Self {
        Self {
            humans: Arc::new(Semaphore::new(HUMAN_STREAM_LIMIT)),
            principals: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn try_acquire(
        self: &Arc<Self>,
        total: Arc<Semaphore>,
        principal: &PrincipalRefV1,
    ) -> Result<Lease, AdmissionFailure> {
        let key = principal.key().map_err(|_| AdmissionFailure::Unavailable)?;
        let mut principals = self
            .principals
            .lock()
            .map_err(|_| AdmissionFailure::Unavailable)?;
        principals.retain(|_, slot| slot.strong_count() > 0);
        let slot = principals
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let slot = Arc::new(Semaphore::new(PRINCIPAL_STREAM_LIMIT));
                principals.insert(key, Arc::downgrade(&slot));
                slot
            });
        // Hold the registry only for these memory-only, nonblocking operations.
        // On a later failure the earlier owned permits roll back immediately.
        let permits = (|| {
            let principal = Arc::clone(&slot)
                .try_acquire_owned()
                .map_err(|_| AdmissionFailure::Capacity)?;
            let human = Arc::clone(&self.humans)
                .try_acquire_owned()
                .map_err(|_| AdmissionFailure::Capacity)?;
            let total = total
                .try_acquire_owned()
                .map_err(|_| AdmissionFailure::Capacity)?;
            Ok((principal, human, total))
        })();
        drop(slot);
        let (principal, human, total) = match permits {
            Ok(permits) => permits,
            Err(error) => {
                principals.retain(|_, slot| slot.strong_count() > 0);
                return Err(error);
            }
        };
        drop(principals);
        Ok(Lease {
            principal: Some(principal),
            human: Some(human),
            total: Some(total),
            admission: Arc::clone(self),
        })
    }
}

pub(super) struct Lease {
    principal: Option<OwnedSemaphorePermit>,
    human: Option<OwnedSemaphorePermit>,
    total: Option<OwnedSemaphorePermit>,
    admission: Arc<Admission>,
}

impl Lease {
    pub(super) fn bind<S>(self, stream: S) -> LeasedStream<S> {
        LeasedStream {
            inner: Box::pin(stream),
            _lease: self,
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.total.take());
        drop(self.human.take());
        drop(self.principal.take());
        // No I/O, async work or timer in Drop. Prune after releasing permits;
        // concurrent admissions retain live slots under this same registry lock.
        if let Ok(mut principals) = self.admission.principals.lock() {
            principals.retain(|_, slot| slot.strong_count() > 0);
        }
    }
}

// The lease is outside the generator, so reaching its final yield or EOF cannot
// release capacity. Axum's SseBody/KeepAliveStream own this wrapper through Body
// destruction, even after poll_next returns None. This bounds admitted Bodies,
// not every HTTP socket; an unpolled Body can retain its principal's four slots.
pub(super) struct LeasedStream<S> {
    inner: Pin<Box<S>>,
    _lease: Lease,
}

impl<S: Stream> Stream for LeasedStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
#[path = "human_streams_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(index: usize) -> PrincipalRefV1 {
        PrincipalRefV1::HumanLocal {
            installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
            account_id: format!("00000000-0000-4000-8000-{index:012}"),
        }
    }

    #[test]
    fn released_principals_are_purged_instead_of_accumulating_keys() {
        let admission = Arc::new(Admission::new());
        let total = Arc::new(Semaphore::new(TOTAL_STREAM_LIMIT));
        let retained = admission
            .try_acquire(Arc::clone(&total), &principal(0))
            .unwrap();
        for index in 1..100 {
            let lease = admission
                .try_acquire(Arc::clone(&total), &principal(index))
                .unwrap();
            assert_eq!(admission.principals.lock().unwrap().len(), 2);
            drop(lease);
            assert_eq!(admission.principals.lock().unwrap().len(), 1);
        }
        drop(retained);
        assert!(admission.principals.lock().unwrap().is_empty());
        assert_eq!(total.available_permits(), 32);
        assert_eq!(admission.humans.available_permits(), 24);
    }

    #[test]
    fn failed_total_admission_removes_new_principal_and_returns_human_permit() {
        let admission = Arc::new(Admission::new());
        let total = Arc::new(Semaphore::new(0));
        for index in 0..100 {
            assert!(matches!(
                admission.try_acquire(Arc::clone(&total), &principal(index)),
                Err(AdmissionFailure::Capacity)
            ));
            assert!(admission.principals.lock().unwrap().is_empty());
            assert_eq!(admission.humans.available_permits(), 24);
        }
    }
}
