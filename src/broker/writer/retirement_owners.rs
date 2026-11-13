//! Process-local ownership of D5 commands, independent of their reply waiter.
//! No sender, journal core, durable record or reconstructed capability is held.
use super::*;
use std::{any::Any, collections::BTreeMap, sync::Mutex};

/// A reference to an already retained input, not a copy of its authority. Drop
/// of this ticket leaves the input and its independent capacity slot intact.
pub(crate) struct RetirementContinuation<I> {
    registry: Arc<RetirementOwners>,
    key: OwnerKey,
    marker: std::marker::PhantomData<fn() -> I>,
}
impl<I> std::fmt::Debug for RetirementContinuation<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetirementContinuation(<retained original input>)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum OwnerPhase {
    Begin,
    Authorize,
    Final,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OwnerKey {
    pub(super) operation: Uuid,
    pub(super) arm: Uuid,
    pub(super) retirement: Uuid,
    pub(super) phase: OwnerPhase,
}

struct Entry {
    commitment: String,
    owner: Arc<dyn Any + Send + Sync>,
    active: Box<dyn Fn() -> JournalResult<bool> + Send + Sync>,
}

pub(super) struct RetirementOwners {
    capacity: usize,
    entries: Mutex<BTreeMap<OwnerKey, Entry>>,
    closed: AtomicBool,
    exited: AtomicBool,
    changed: Arc<tokio::sync::Notify>,
    _lease: Backend,
    #[cfg(test)]
    reply_gate: Mutex<Option<TestReplyGate>>,
    #[cfg(test)]
    writer_join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

enum State<I, O> {
    Queued(I),
    Running,
    Complete {
        output: O,
        _input: Option<I>,
    },
    Refused {
        error: JournalError,
        input: I,
    },
    Unknown {
        error: JournalError,
        _input: Option<I>,
    },
    Taken,
}

pub(super) struct Owner<I, O> {
    state: Mutex<State<I, O>>,
    changed: Arc<tokio::sync::Notify>,
}

pub(super) enum Work<O> {
    Complete(O),
    Refused(JournalError),
    Unknown(JournalError),
}

/// Read and consume a completed outcome under the same registry/state locks.
/// A queued/running owner remains authoritative and must not be mistaken for
/// an absent operation by a reconciler that might otherwise admit a new Final.
pub(super) enum OwnerRecovery<O> {
    Absent,
    InFlight,
    Completed(O),
}

struct InFlight<I, O> {
    owner: Arc<Owner<I, O>>,
    input: Option<I>,
    completed: bool,
}

impl<I, O> Drop for InFlight<I, O> {
    fn drop(&mut self) {
        if !self.completed {
            // A panicking writer does not abandon the sole remaining input.
            // Recovering the poisoned lock here retains ownership only; reads
            // still treat the operation as unknown and never mint an output.
            let mut state = self.owner.state.lock().unwrap_or_else(|e| e.into_inner());
            *state = State::Unknown {
                error: JournalError::Io,
                _input: self.input.take(),
            };
            self.owner.changed.notify_waiters();
        }
    }
}

impl<I, O> Owner<I, O> {
    pub(super) fn process(
        self: &Arc<Self>,
        action: impl FnOnce(&mut Option<I>) -> Work<O>,
    ) -> JournalResult<()> {
        let input = {
            let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
            match std::mem::replace(&mut *state, State::Running) {
                State::Queued(input) => input,
                old => {
                    *state = old;
                    return Err(JournalError::Conflict);
                }
            }
        };
        let mut guard = InFlight {
            owner: Arc::clone(self),
            input: Some(input),
            completed: false,
        };
        let next = match action(&mut guard.input) {
            Work::Complete(output) => State::Complete {
                output,
                _input: guard.input.take(),
            },
            Work::Refused(error) => match guard.input.take() {
                Some(input) => State::Refused { error, input },
                None => State::Unknown {
                    error: JournalError::Corrupt,
                    _input: None,
                },
            },
            Work::Unknown(error) => State::Unknown {
                error,
                _input: guard.input.take(),
            },
        };
        // Publish the retained outcome before notifying the waiter. A lost
        // notification, including send Ok followed by receiver Drop, owns no
        // unique capability and cannot erase this continuation.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = next;
        guard.completed = true;
        self.changed.notify_waiters();
        Ok(())
    }
}

impl RetirementOwners {
    pub(super) fn new(capacity: usize, lease: Backend) -> Self {
        // Reuse the writer's validated ordinary queue capacity (1..=256).
        // This is a reservation of ownership slots, not added journal bytes,
        // tenant quota, Cold completion capacity or guardian authority.
        Self {
            capacity,
            entries: Mutex::new(BTreeMap::new()),
            closed: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            changed: Arc::new(tokio::sync::Notify::new()),
            _lease: lease,
            #[cfg(test)]
            reply_gate: Mutex::new(None),
            #[cfg(test)]
            writer_join: Mutex::new(None),
        }
    }

    pub(super) fn admit<I: Send + 'static, O: Send + 'static>(
        self: &Arc<Self>,
        key: OwnerKey,
        commitment: String,
        input: I,
    ) -> Result<Arc<Owner<I, O>>, RetirementFailure<I>> {
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(_) => {
                return Err(RetirementFailure::NotQueued {
                    error: JournalError::Corrupt,
                    input,
                });
            }
        };
        if self.closed.load(Ordering::Acquire) || self.exited.load(Ordering::Acquire) {
            return Err(RetirementFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        if let Some(existing) = entries.get(&key) {
            return Err(RetirementFailure::NotQueued {
                error: if existing.commitment == commitment {
                    JournalError::Busy
                } else {
                    JournalError::Conflict
                },
                input,
            });
        }
        if entries.len() >= self.capacity {
            return Err(RetirementFailure::NotQueued {
                error: JournalError::Busy,
                input,
            });
        }
        let owner = Arc::new(Owner {
            state: Mutex::new(State::Queued(input)),
            changed: Arc::clone(&self.changed),
        });
        let observed = Arc::clone(&owner);
        entries.insert(
            key,
            Entry {
                commitment,
                owner: owner.clone(),
                active: Box::new(move || {
                    let state = observed.state.lock().map_err(|_| JournalError::Corrupt)?;
                    Ok(matches!(*state, State::Queued(_) | State::Running))
                }),
            },
        );
        Ok(owner)
    }

    fn owner<I: Send + 'static, O: Send + 'static>(
        entry: &Entry,
    ) -> JournalResult<Arc<Owner<I, O>>> {
        Arc::clone(&entry.owner)
            .downcast()
            .map_err(|_| JournalError::Conflict)
    }

    pub(super) fn unqueue<I: Send + 'static, O: Send + 'static>(
        &self,
        key: OwnerKey,
    ) -> JournalResult<I> {
        let mut entries = self.entries.lock().map_err(|_| JournalError::Corrupt)?;
        let owner = Self::owner::<I, O>(entries.get(&key).ok_or(JournalError::Conflict)?)?;
        let mut state = owner.state.lock().map_err(|_| JournalError::Corrupt)?;
        match std::mem::replace(&mut *state, State::Taken) {
            State::Queued(input) => {
                entries.remove(&key);
                self.changed.notify_waiters();
                Ok(input)
            }
            old => {
                *state = old;
                Err(JournalError::Conflict)
            }
        }
    }

    pub(super) fn take<I: Send + 'static, O: Send + 'static>(
        self: &Arc<Self>,
        key: OwnerKey,
    ) -> Result<Option<O>, RetirementFailure<I>> {
        match self.recover(key)? {
            OwnerRecovery::Completed(output) => Ok(Some(output)),
            // Legacy bounded retry callers use None only to keep waiting.
            OwnerRecovery::Absent | OwnerRecovery::InFlight => Ok(None),
        }
    }

    pub(super) fn recover<I: Send + 'static, O: Send + 'static>(
        self: &Arc<Self>,
        key: OwnerKey,
    ) -> Result<OwnerRecovery<O>, RetirementFailure<I>> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| RetirementFailure::Unknown {
                error: JournalError::Corrupt,
            })?;
        let Some(entry) = entries.get(&key) else {
            return Ok(OwnerRecovery::Absent);
        };
        let owner =
            Self::owner::<I, O>(entry).map_err(|error| RetirementFailure::Unknown { error })?;
        let mut state = owner.state.lock().map_err(|_| RetirementFailure::Unknown {
            error: JournalError::Corrupt,
        })?;
        match std::mem::replace(&mut *state, State::Taken) {
            State::Complete { output, _input } => {
                entries.remove(&key);
                self.changed.notify_waiters();
                Ok(OwnerRecovery::Completed(output))
            }
            State::Refused { error, input } => {
                *state = State::Refused { error, input };
                Err(RetirementFailure::Refused {
                    error,
                    continuation: RetirementContinuation {
                        registry: Arc::clone(self),
                        key,
                        marker: std::marker::PhantomData,
                    },
                })
            }
            old @ (State::Queued(_) | State::Running) => {
                *state = old;
                // Retain the exact owner/input even when no writer can make
                // progress. Settled outcomes above stay recoverable on exit.
                if self.exited.load(Ordering::Acquire) {
                    Err(RetirementFailure::Unknown {
                        error: JournalError::Io,
                    })
                } else {
                    Ok(OwnerRecovery::InFlight)
                }
            }
            State::Unknown { error, _input } => {
                *state = State::Unknown { error, _input };
                Err(RetirementFailure::Unknown { error })
            }
            State::Taken => {
                *state = State::Taken;
                Err(RetirementFailure::Unknown {
                    error: JournalError::Conflict,
                })
            }
        }
    }

    pub(super) fn retry<I: Send + 'static, O: Send + 'static>(
        self: &Arc<Self>,
        continuation: RetirementContinuation<I>,
    ) -> Result<(OwnerKey, Arc<Owner<I, O>>), RetirementFailure<I>> {
        if !Arc::ptr_eq(self, &continuation.registry) {
            return Err(RetirementFailure::Unknown {
                error: JournalError::Conflict,
            });
        }
        let entries = self
            .entries
            .lock()
            .map_err(|_| RetirementFailure::Unknown {
                error: JournalError::Corrupt,
            })?;
        let entry = entries
            .get(&continuation.key)
            .ok_or(RetirementFailure::Unknown {
                error: JournalError::Conflict,
            })?;
        let owner =
            Self::owner::<I, O>(entry).map_err(|error| RetirementFailure::Unknown { error })?;
        let mut state = owner.state.lock().map_err(|_| RetirementFailure::Unknown {
            error: JournalError::Corrupt,
        })?;
        if self.exited.load(Ordering::Acquire) {
            return Err(RetirementFailure::Unknown {
                error: JournalError::Io,
            });
        }
        match std::mem::replace(&mut *state, State::Running) {
            State::Refused { input, .. } => {
                *state = State::Queued(input);
                drop(state);
                Ok((continuation.key, owner))
            }
            previous => {
                *state = previous;
                Err(RetirementFailure::Unknown {
                    error: JournalError::Conflict,
                })
            }
        }
    }

    pub(super) fn retry_not_queued<I: Send + 'static, O: Send + 'static>(
        self: &Arc<Self>,
        key: OwnerKey,
        error: JournalError,
    ) -> RetirementFailure<I> {
        let result = (|| {
            let entries = self.entries.lock().map_err(|_| JournalError::Corrupt)?;
            let owner = Self::owner::<I, O>(entries.get(&key).ok_or(JournalError::Conflict)?)?;
            let mut state = owner.state.lock().map_err(|_| JournalError::Corrupt)?;
            match std::mem::replace(&mut *state, State::Running) {
                // This retry originated in the explicit Cold refusal. The
                // command was not queued; preserve that original refusal,
                // rather than classify a queue/IO error as a new Cold guard.
                State::Queued(input) => {
                    *state = State::Refused {
                        error: JournalError::Busy,
                        input,
                    };
                    Ok(())
                }
                previous => {
                    *state = previous;
                    Err(JournalError::Conflict)
                }
            }
        })();
        match result {
            Ok(()) => RetirementFailure::Unknown { error },
            Err(error) => RetirementFailure::Unknown { error },
        }
    }

    pub(super) fn close(&self) -> JournalResult<()> {
        let _entries = self.entries.lock().map_err(|_| JournalError::Corrupt)?;
        self.closed.store(true, Ordering::Release);
        self.changed.notify_waiters();
        Ok(())
    }

    pub(super) fn has_retained(&self) -> bool {
        self.entries
            .lock()
            .map(|entries| !entries.is_empty())
            .unwrap_or(true)
    }

    pub(super) fn exited(&self) {
        self.exited.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    pub(super) async fn drain(&self) -> JournalResult<()> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (active, empty) = {
                let entries = self.entries.lock().map_err(|_| JournalError::Corrupt)?;
                let mut active = false;
                for entry in entries.values() {
                    active |= (entry.active)()?;
                }
                (active, entries.is_empty())
            };
            if !active {
                return if empty {
                    Ok(())
                } else {
                    Err(JournalError::Interrupted)
                };
            }
            if self.exited.load(Ordering::Acquire) {
                return Err(JournalError::Io);
            }
            changed.await;
        }
    }
}

#[cfg(test)]
struct TestReplyGate {
    phase: OwnerPhase,
    after_send: bool,
    entered: oneshot::Sender<Option<bool>>,
    release: std::sync::mpsc::Receiver<()>,
}
#[cfg(test)]
impl RetirementOwners {
    #[cfg(test)]
    pub(super) fn retain_writer_join_for_test(&self, join: std::thread::JoinHandle<()>) {
        // A newly constructed registry is recorded exactly once, directly
        // after the actual successful Builder::spawn. No ClientState/sender.
        let mut stored = self.writer_join.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            stored.is_none(),
            "one actual writer thread per test registry"
        );
        *stored = Some(join);
    }
    #[cfg(test)]
    pub(super) fn take_writer_join_for_test(&self) -> JournalResult<std::thread::JoinHandle<()>> {
        self.writer_join
            .lock()
            .map_err(|_| JournalError::Corrupt)?
            .take()
            .ok_or(JournalError::Conflict)
    }
    #[cfg(test)]
    pub(super) fn retained_stops_for_test(&self) -> JournalResult<(StopJournal, u32)> {
        if self.has_retained() {
            return Err(JournalError::Busy);
        }
        let Backend::Full(journal) = &self._lease else {
            return Err(JournalError::InvalidInput);
        };
        Ok((
            StopJournal::from_full(journal),
            u32::try_from(self.capacity).map_err(|_| JournalError::InvalidInput)?,
        ))
    }
    pub(super) fn pause_reply_for_test(
        &self,
        phase: OwnerPhase,
        after_send: bool,
    ) -> (oneshot::Receiver<Option<bool>>, std::sync::mpsc::Sender<()>) {
        let (entered, receive) = oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let mut gate = self.reply_gate.lock().unwrap();
        assert!(gate.is_none(), "one test barrier per actor");
        *gate = Some(TestReplyGate {
            phase,
            after_send,
            entered,
            release: resume,
        });
        (receive, release)
    }
    pub(super) fn reply_boundary_for_test(
        &self,
        phase: OwnerPhase,
        after_send: bool,
        sent: Option<bool>,
    ) {
        let gate = {
            let mut installed = self.reply_gate.lock().unwrap();
            if installed
                .as_ref()
                .is_some_and(|gate| gate.phase == phase && gate.after_send == after_send)
            {
                installed.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(sent);
            // Infrastructure watchdog only, no product deadline is changed.
            let _ = gate.release.recv_timeout(Duration::from_secs(30));
        }
    }
}

pub(super) struct WriterExit(pub(super) Arc<RetirementOwners>);
impl Drop for WriterExit {
    fn drop(&mut self) {
        self.0.exited();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    // These markers exercise ownership/capacity only. They construct no
    // Unused, Pending, Authorized, Q, join or durable retirement evidence.
    struct Marker(Arc<AtomicUsize>);
    impl Drop for Marker {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn marker() -> (Marker, Arc<AtomicUsize>) {
        let drops = Arc::new(AtomicUsize::new(0));
        (Marker(Arc::clone(&drops)), drops)
    }
    fn key() -> OwnerKey {
        OwnerKey {
            operation: Uuid::new_v4(),
            arm: Uuid::new_v4(),
            retirement: Uuid::new_v4(),
            phase: OwnerPhase::Begin,
        }
    }
    fn registry(capacity: usize) -> (tempfile::TempDir, Arc<RetirementOwners>) {
        let (root, journal) = super::super::tests::private_journal();
        (
            root,
            Arc::new(RetirementOwners::new(capacity, Backend::Full(journal))),
        )
    }

    #[test]
    fn queued_owner_reports_unknown_after_writer_exit_without_releasing_input() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "fixed".into(), input)
            .unwrap();
        assert!(matches!(
            registry.recover::<Marker, Marker>(key),
            Ok(OwnerRecovery::InFlight)
        ));

        registry.exited();
        let recovered = registry.recover::<Marker, Marker>(key);
        assert!(registry.has_retained());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        {
            let entries = registry.entries.lock().unwrap();
            let entry = entries
                .get(&key)
                .expect("the original entry stays retained");
            let retained = RetirementOwners::owner::<Marker, Marker>(entry).unwrap();
            assert!(Arc::ptr_eq(&owner, &retained));
        }
        assert!(
            matches!(
                recovered,
                Err(RetirementFailure::Unknown {
                    error: JournalError::Io
                })
            ),
            "a queued retirement cannot progress after its writer exited"
        );
    }

    #[test]
    fn completed_owner_still_yields_its_output_after_writer_exit() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "fixed".into(), input)
            .unwrap();
        owner
            .process(|input| Work::Complete(input.take().unwrap()))
            .unwrap();

        registry.exited();
        let output = match registry.recover::<Marker, Marker>(key) {
            Ok(OwnerRecovery::Completed(output)) => output,
            _ => panic!("a settled output stays recoverable after writer exit"),
        };
        assert!(!registry.has_retained());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(output);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn refused_input_stays_retained_but_cannot_retry_after_writer_exit() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "fixed".into(), input)
            .unwrap();
        owner
            .process(|_| Work::Refused(JournalError::Busy))
            .unwrap();

        registry.exited();
        let continuation = match registry.recover::<Marker, Marker>(key) {
            Err(RetirementFailure::Refused {
                error: JournalError::Busy,
                continuation,
            }) => continuation,
            _ => panic!("the original refusal stays recoverable after writer exit"),
        };
        assert!(matches!(
            registry.retry::<Marker, Marker>(continuation),
            Err(RetirementFailure::Unknown {
                error: JournalError::Io
            })
        ));
        assert!(registry.has_retained());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let entries = registry.entries.lock().unwrap();
        let entry = entries
            .get(&key)
            .expect("the original entry stays retained");
        let retained = RetirementOwners::owner::<Marker, Marker>(entry).unwrap();
        assert!(Arc::ptr_eq(&owner, &retained));
    }

    #[tokio::test]
    async fn actual_input_and_capacity_survive_reply_send_ok_then_receiver_drop() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "exact".into(), input)
            .unwrap();
        owner
            .process(|input| Work::Complete(input.take().unwrap()))
            .unwrap();
        let (reply, receive) = oneshot::channel();
        assert!(reply.send(()).is_ok());
        drop(receive);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let (second, _drops) = marker();
        let failure = registry
            .admit::<Marker, Marker>(self::key(), "other".into(), second)
            .err()
            .unwrap();
        assert!(matches!(
            failure,
            RetirementFailure::NotQueued {
                error: JournalError::Busy,
                ..
            }
        ));
        assert_eq!(registry.drain().await, Err(JournalError::Interrupted));
        let output = registry.take::<Marker, Marker>(key).unwrap().unwrap();
        assert!(registry.take::<Marker, Marker>(key).unwrap().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(output);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(registry.drain().await, Ok(()));
    }

    #[tokio::test]
    async fn refused_ticket_drop_close_and_explicit_retry_preserve_the_same_input() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "fixed".into(), input)
            .unwrap();
        owner
            .process(|_| Work::Refused(JournalError::Busy))
            .unwrap();
        let failure = registry.take::<Marker, Marker>(key).err().unwrap();
        assert!(matches!(failure, RetirementFailure::Refused { .. }));
        drop(failure);
        assert!(registry.has_retained());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        registry.close().unwrap();
        let continuation = match registry.take::<Marker, Marker>(key) {
            Err(RetirementFailure::Refused { continuation, .. }) => continuation,
            _ => panic!("the exact original refusal stays recoverable"),
        };
        let (_, retried) = registry.retry::<Marker, Marker>(continuation).unwrap();
        assert!(Arc::ptr_eq(&owner, &retried));
        retried
            .process(|input| Work::Complete(input.take().unwrap()))
            .unwrap();
        drop(registry.take::<Marker, Marker>(key).unwrap().unwrap());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(registry.drain().await, Ok(()));
    }

    #[tokio::test]
    async fn panicking_action_retains_original_without_a_new_queue_slot_or_output() {
        let (_root, registry) = registry(1);
        let key = key();
        let (input, drops) = marker();
        let owner = registry
            .admit::<Marker, Marker>(key, "fixed".into(), input)
            .unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = owner.process(|_| panic!("test-only action failure"));
            }))
            .is_err()
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(matches!(
            registry.take::<Marker, Marker>(key),
            Err(RetirementFailure::Unknown {
                error: JournalError::Io
            })
        ));
        assert!(registry.has_retained());
        registry.close().unwrap();
        assert_eq!(registry.drain().await, Err(JournalError::Interrupted));
    }
}
