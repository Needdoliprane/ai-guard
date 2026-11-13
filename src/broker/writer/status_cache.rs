//! Last-confirmed diagnostics, never a journal reader or execution capability.
//! The actor alone publishes under the reply barrier; readers never enqueue.
use super::*;
use crate::broker::{
    journal::status_projection::LastConfirmedStatus,
    lifecycle::{BrokerCheckpointV3, LocalCommandRequestV1},
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, Weak},
};

const MAX_RETAINED_JOBS: usize = 256;
const MAX_RETAINED_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADS: usize = 100;

/// A partial window of acknowledged cache metadata, never a journal inventory
/// or an execution capability. Only the cache can construct this observation.
pub(crate) struct LastConfirmedHeads {
    entries: Vec<(Uuid, RevisionRefV1)>,
    has_more: bool,
}
impl LastConfirmedHeads {
    pub(crate) fn entries(&self) -> &[(Uuid, RevisionRefV1)] {
        &self.entries
    }
    /// More entries exist in this retained cache window. Even false does not
    /// establish complete journal coverage: cold/evicted jobs are not scanned.
    pub(crate) fn has_more(&self) -> bool {
        self.has_more
    }
}

pub(super) struct StatusCache {
    state: Mutex<State>,
}
struct State {
    alive: bool,
    tainted: bool,
    jobs: BTreeMap<Uuid, LastConfirmedStatus>,
    bytes: usize,
    job_limit: usize,
    byte_limit: usize,
    service_guards: Option<(Weak<AtomicBool>, Weak<AtomicBool>)>,
}
pub(super) enum Update<'a> {
    None,
    Clear,
    Replace(LastConfirmedStatus),
    Lifecycle {
        operation: Uuid,
        head: RevisionRefV1,
        next: &'a BrokerCheckpointV3,
    },
    Head {
        operation: Uuid,
        head: RevisionRefV1,
    },
    Local {
        operation: Uuid,
        head: RevisionRefV1,
        request: &'a LocalCommandRequestV1,
    },
}
impl StatusCache {
    pub(super) fn new(jobs: u32, bytes: u64) -> Self {
        Self {
            state: Mutex::new(State {
                alive: true,
                tainted: false,
                jobs: BTreeMap::new(),
                bytes: 0,
                job_limit: (jobs as usize).min(MAX_RETAINED_JOBS),
                byte_limit: usize::try_from(bytes.min(MAX_RETAINED_BYTES as u64))
                    .unwrap_or(MAX_RETAINED_BYTES),
                service_guards: None,
            }),
        }
    }
    pub(super) fn read(&self, operation: Uuid) -> JournalResult<Option<LastConfirmedStatus>> {
        if operation.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        if !state.alive || !state.service_available() {
            state.tainted = true;
            state.jobs.clear();
            state.bytes = 0;
            return Err(JournalError::Io);
        }
        if state.tainted {
            return Err(JournalError::Corrupt);
        }
        Ok(state.jobs.get(&operation).cloned())
    }
    pub(super) fn read_heads(&self, limit: usize) -> JournalResult<LastConfirmedHeads> {
        if !(1..=MAX_HEADS).contains(&limit) {
            return Err(JournalError::InvalidInput);
        }
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        if !state.alive || !state.service_available() {
            state.tainted = true;
            state.jobs.clear();
            state.bytes = 0;
            return Err(JournalError::Io);
        }
        if state.tainted {
            return Err(JournalError::Corrupt);
        }
        if state.jobs.is_empty() {
            return Err(JournalError::Interrupted);
        }
        let entries = state
            .jobs
            .iter()
            .take(limit)
            .map(|(operation, status)| {
                let head = status.entry().head().ok_or(JournalError::Corrupt)?;
                Ok((*operation, head.clone()))
            })
            .collect::<JournalResult<Vec<_>>>()?;
        Ok(LastConfirmedHeads {
            entries,
            has_more: state.jobs.len() > limit,
        })
    }
    pub(super) fn bind_service(&self, fenced: Weak<AtomicBool>, stopping: Weak<AtomicBool>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &state.service_guards {
            Some((known_fence, known_stop))
                if !Weak::ptr_eq(known_fence, &fenced) || !Weak::ptr_eq(known_stop, &stopping) =>
            {
                state.tainted = true;
                state.jobs.clear();
                state.bytes = 0;
            }
            Some(_) => {}
            None => state.service_guards = Some((fenced, stopping)),
        }
    }
    pub(super) fn invalidate(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.tainted = true;
        state.jobs.clear();
        state.bytes = 0;
    }
    pub(super) fn close(&self) {
        self.invalidate();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .alive = false;
    }
    pub(super) fn reply<T>(
        &self,
        reply: Reply<T>,
        result: JournalResult<T>,
        update: Update<'_>,
        mutation: bool,
        strict_failed: bool,
    ) -> Result<(), JournalResult<T>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if strict_failed
            || !state.service_available()
            || matches!(
                &result,
                Err(JournalError::Io
                    | JournalError::Corrupt
                    | JournalError::UnsafeFilesystem
                    | JournalError::ControlSecret)
            )
        {
            state.tainted = true;
            state.jobs.clear();
            state.bytes = 0;
        }
        let succeeded = result.is_ok();
        // send is nonblocking and performs no IO. Holding this bounded metadata
        // mutex until publication prevents a reader seeing a head before ACK.
        match reply.send(result) {
            Ok(()) => {
                if succeeded && state.alive && !state.tainted {
                    state.apply(update);
                }
                Ok(())
            }
            Err(result) => {
                if mutation {
                    state.tainted = true;
                    state.jobs.clear();
                    state.bytes = 0;
                }
                Err(result)
            }
        }
    }
}
impl State {
    fn service_available(&self) -> bool {
        self.service_guards
            .as_ref()
            .is_none_or(|(fenced, stopping)| {
                fenced
                    .upgrade()
                    .is_some_and(|fenced| !fenced.load(std::sync::atomic::Ordering::SeqCst))
                    && stopping
                        .upgrade()
                        .is_some_and(|stopping| !stopping.load(std::sync::atomic::Ordering::SeqCst))
            })
    }
    fn remove(&mut self, operation: Uuid) -> Option<LastConfirmedStatus> {
        let previous = self.jobs.remove(&operation)?;
        self.bytes = self.bytes.saturating_sub(previous.retained_bytes());
        Some(previous)
    }
    fn insert(&mut self, value: LastConfirmedStatus) {
        if value.entry().kind() != super::super::journal::JournalEntryKindV1::Accepted {
            return;
        }
        let operation = value.entry().operation_id();
        self.remove(operation);
        let bytes = value.retained_bytes();
        if self.jobs.len() < self.job_limit && bytes <= self.byte_limit.saturating_sub(self.bytes) {
            self.bytes += bytes;
            self.jobs.insert(operation, value);
        }
        // Capacity refusal is a cache miss, never an invented empty job. The
        // original strict path still supplies a bounded diagnostic response.
    }
    fn apply(&mut self, update: Update<'_>) {
        match update {
            Update::None => {}
            Update::Clear => {
                self.jobs.clear();
                self.bytes = 0;
            }
            Update::Replace(value) => self.insert(value),
            Update::Lifecycle {
                operation,
                head,
                next,
            } => {
                if let Some(mut value) = self.remove(operation) {
                    if value.update_lifecycle(head, next).is_ok() {
                        self.insert(value);
                    }
                }
            }
            Update::Head { operation, head } => {
                if let Some(mut value) = self.remove(operation) {
                    // A V1 job can be promoted by its first outbox mutation.
                    // A head alone cannot describe its newly present V2 data.
                    if value.entry().pending_rpc_count().is_some()
                        && value.update_head(head).is_ok()
                    {
                        self.insert(value);
                    }
                }
            }
            Update::Local {
                operation,
                head,
                request,
            } => {
                if let Some(mut value) = self.remove(operation) {
                    if value.entry().pending_rpc_count().is_some()
                        && value.update_local(head, request).is_ok()
                    {
                        self.insert(value);
                    }
                }
            }
        }
    }
}

pub(super) struct ActorLifetime(pub(super) Arc<StatusCache>);
impl Drop for ActorLifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// A queued strict call whose reply waiter disappears has an unknown outcome.
/// This retains no sender/lease and never cancels the actor's accepted work.
pub(super) struct WaiterGuard {
    cache: Arc<StatusCache>,
    acknowledged: bool,
}
impl WaiterGuard {
    pub(super) fn new(cache: Arc<StatusCache>) -> Self {
        Self {
            cache,
            acknowledged: false,
        }
    }
    pub(super) fn acknowledged(&mut self) {
        self.acknowledged = true;
    }
}
impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.cache.invalidate();
        }
    }
}
