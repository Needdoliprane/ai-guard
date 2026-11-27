//! Broker composition of SendSessionMessage: the trusted-control caller that
//! wires the durable effect points of `audit::session_message` to the exact
//! owned conversation channel of an admitted worker.
//!
//! Lifecycle composition (SESSION_CONTROL, BROKER_SUPERVISION):
//! - after the supervised admission resolves the exact channel facts, the
//!   trusted control binds them durably (`bind_session_conversation_scoped`)
//!   before the owned stdio channel is attached here;
//! - a queued instruction reaches the worker only through the durable latch
//!   (`issue_session_message_emission_scoped`) before the first frame byte,
//!   then the observed complete write (`record_session_message_transmitted_scoped`)
//!   and the exact correlated official response
//!   (`record_session_message_acknowledged_scoped`);
//! - STOP closes the right to emit durably first
//!   (`close_session_conversation_scoped`), then closes the channel.
//!
//! This composition never spawns a provider client, never opens a network
//! socket, never enables Bridge and never loosens a preset: it receives the
//! stdio pipe the trusted runtime already owns under network None and the
//! approved Rust egress direction. It never restarts a worker, never resets a
//! quota or time budget, and never resends after a possible emission: delivery
//! uncertainty is retained by the durable latch. Restart, attempt or owner
//! changes never retarget a pinned message: the store revalidates the exact
//! binding fingerprint at the effect point and this composer refuses any
//! divergence instead of writing.
//!
//! The production bridge from the supervised broker admission (broker process)
//! to this gateway-side owned-source composition is a separate integration
//! step; the behavior below is proven end to end against a real owned Store
//! and a controlled stdio fixture child, not a provider (NOT_RUN).

use crate::audit::session_message::SessionMessageFailureV1;
use crate::audit::{HumanSourceAccess, Store};
use crate::control::session_control::{
    BindSessionConversationV1, CloseSessionConversationV1, IssueSessionMessageEmissionV1,
    SessionConversationBindingV1, SessionConversationChannelV1, SessionConversationStatusV1,
    SessionMessageAckV1, SessionMessageReceiptV1, canonical_uuid,
};
use crate::control::session_message::{
    AckObservationV1, ConversationAdapterV1, OutcomeObservationV1, StdioConversation,
};
use crate::human_authority::{
    HumanAuthorityFailure, HumanSourcesReadEpoch, HumanSourcesWriteEpoch, OwnedHumanSources,
    OwnedSourceToken,
};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Upper bound of response frames reconciled for one issued turn. Beyond it
/// the turn keeps its truthful durable status and the channel is freed; no
/// instruction is ever resent.
const MAX_RECONCILE_FRAMES: usize = 256;
const READ_RETIREMENT_CHECK_INTERVAL: Duration = Duration::from_millis(50);

/// Bounded acquisition of the owned-source epoch gate. The gate is a real
/// single-writer exclusion: a concurrent short transaction (or STOP) may hold
/// it momentarily. Acquisition retries only the transient `GateBusy` cause,
/// never another failure, and never extends a binding or protocol deadline.
const EPOCH_RETRY_DEADLINE: Duration = Duration::from_secs(10);
const EPOCH_RETRY_INTERVAL: Duration = Duration::from_millis(5);

/// The mutable correlation state and owned channel of an admitted conversation.
struct BoundConversation {
    adapter: ConversationAdapterV1,
    channel: StdioConversation,
    thread_id: String,
}

/// The immutable durable binding and the signal to retire its owned channel.
struct ConversationSlot {
    binding: SessionConversationBindingV1,
    fingerprint: String,
    retiring: AtomicBool,
    bound: Mutex<BoundConversation>,
    predecessor: PendingPredecessor<ConversationSlot>,
}

/// Owns the one exact predecessor whose reap gates use of the current slot.
/// The entry is cleared only after the caller confirms reap completed.
struct PendingPredecessor<T> {
    slot: Mutex<Option<Arc<T>>>,
}

impl<T> Default for PendingPredecessor<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }
}

impl<T> PendingPredecessor<T> {
    fn install(&self, predecessor: Arc<T>) -> anyhow::Result<()> {
        let mut slot = self.slot.lock().expect("conversation predecessor");
        anyhow::ensure!(slot.is_none(), "previous conversation reap is pending");
        *slot = Some(predecessor);
        Ok(())
    }

    fn reap(&self, reap: impl FnOnce(&T) -> anyhow::Result<()>) -> anyhow::Result<()> {
        let mut slot = self.slot.lock().expect("conversation predecessor");
        let Some(predecessor) = slot.as_ref() else {
            return Ok(());
        };
        reap(predecessor)?;
        slot.take();
        Ok(())
    }

    #[cfg(test)]
    fn is_pending(&self) -> bool {
        self.slot
            .lock()
            .expect("conversation predecessor")
            .is_some()
    }
}

impl ConversationSlot {
    fn is_retiring(&self) -> bool {
        self.retiring.load(Ordering::Acquire)
    }

    fn next_response(
        &self,
        channel: &mut StdioConversation,
        deadline: Instant,
    ) -> io::Result<String> {
        loop {
            if self.is_retiring() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let result = channel.read_line(remaining.min(READ_RETIREMENT_CHECK_INTERVAL));
            if self.is_retiring() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match result {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                result => return result,
            }
        }
    }
}

#[cfg(test)]
struct BeforeWritePause {
    min_written: usize,
    run: Box<dyn FnOnce(usize) + Send>,
}

/// The trusted-control composition driving admitted session conversations.
/// Holds no durable state itself: every fact is recorded through the owned
/// Store under its write epoch before (or exactly at) the observed effect.
pub struct SessionConversationComposerV1<'a> {
    store: &'a Store,
    owner: &'a OwnedHumanSources,
    source: &'a OwnedSourceToken,
    read_deadline: Duration,
    sessions: Mutex<BTreeMap<String, Arc<ConversationSlot>>>,
    #[cfg(test)]
    before_write: Mutex<Option<BeforeWritePause>>,
    #[cfg(test)]
    before_ack: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    stop_slot_observer: Option<std::sync::mpsc::Sender<()>>,
}

fn channel_matches_binding(
    channel: &SessionConversationChannelV1,
    binding: &SessionConversationBindingV1,
) -> bool {
    binding.broker_id.as_deref() == Some(channel.broker_id.as_str())
        && binding.broker_incarnation.as_deref() == Some(channel.broker_incarnation.as_str())
        && binding.worker_id.as_deref() == Some(channel.worker_id.as_str())
        && binding.attempt_id.as_deref() == Some(channel.attempt_id.as_str())
        && binding.runtime_instance.as_deref() == Some(channel.runtime_instance.as_str())
        && binding.provider.as_deref() == Some(channel.provider.as_str())
        && binding.protocol.as_deref() == Some(channel.protocol.as_str())
}

#[cfg(test)]
#[path = "conversation_reap_tests.rs"]
mod reap_tests;

impl<'a> SessionConversationComposerV1<'a> {
    /// Compose over one retained owned source. The read deadline bounds the
    /// response reads for one turn; it never extends the binding's expiry.
    pub fn new(
        store: &'a Store,
        owner: &'a OwnedHumanSources,
        source: &'a OwnedSourceToken,
        read_deadline: Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !read_deadline.is_zero() && read_deadline <= Duration::from_secs(60),
            "conversation read deadline must be bounded and non-zero"
        );
        Ok(Self {
            store,
            owner,
            source,
            read_deadline,
            sessions: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            before_write: Mutex::new(None),
            #[cfg(test)]
            before_ack: Mutex::new(None),
            #[cfg(test)]
            stop_slot_observer: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_before_write_for_test(
        &mut self,
        min_written: usize,
        pause: impl FnOnce(usize) + Send + 'static,
    ) {
        *self.before_write.get_mut().unwrap() = Some(BeforeWritePause {
            min_written,
            run: Box::new(pause),
        });
    }

    #[cfg(test)]
    fn pause_before_write_for_test(&self, written: usize) {
        let mut pending = self.before_write.lock().unwrap();
        if pending
            .as_ref()
            .is_some_and(|pause| written >= pause.min_written)
        {
            let pause = pending.take().unwrap();
            drop(pending);
            (pause.run)(written);
        }
    }

    #[cfg(test)]
    pub(crate) fn set_before_ack_for_test(&mut self, pause: impl FnOnce() + Send + 'static) {
        *self.before_ack.get_mut().unwrap() = Some(Box::new(pause));
    }

    #[cfg(test)]
    pub(crate) fn observe_stop_slot_for_test(&mut self, observer: std::sync::mpsc::Sender<()>) {
        self.stop_slot_observer = Some(observer);
    }

    /// Write epoch with bounded transient-busy retry (STOP keeps priority over
    /// the business drain by closing the right to emit at the effect point).
    fn write_epoch(&self) -> anyhow::Result<HumanSourcesWriteEpoch> {
        self.write_epoch_until(Instant::now() + EPOCH_RETRY_DEADLINE)
    }

    fn write_epoch_until(&self, deadline: Instant) -> anyhow::Result<HumanSourcesWriteEpoch> {
        Self::retry_write_epoch_until(deadline, || self.owner.try_write_epoch())
    }

    fn acknowledgement_epoch(
        &self,
        slot: &ConversationSlot,
    ) -> anyhow::Result<Option<HumanSourcesWriteEpoch>> {
        Self::retry_write_epoch_until(Instant::now() + EPOCH_RETRY_DEADLINE, || {
            if slot.is_retiring() {
                return Ok(None);
            }
            let epoch = self.owner.try_write_epoch();
            // Retirement is published under the write epoch. A successful
            // acquisition can commit the ACK only if it won that ordering.
            if slot.is_retiring() {
                return Ok(None);
            }
            epoch.map(Some)
        })
    }

    fn retry_write_epoch_until<T>(
        deadline: Instant,
        mut try_epoch: impl FnMut() -> Result<T, HumanAuthorityFailure>,
    ) -> anyhow::Result<T> {
        loop {
            match try_epoch() {
                Ok(result) => return Ok(result),
                Err(HumanAuthorityFailure::GateBusy) if Instant::now() < deadline => {
                    std::thread::sleep(
                        EPOCH_RETRY_INTERVAL
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Read epoch with the same bounded transient-busy retry.
    fn read_epoch(&self) -> anyhow::Result<HumanSourcesReadEpoch> {
        let start = Instant::now();
        loop {
            match self.owner.try_read_epoch() {
                Ok(epoch) => return Ok(epoch),
                Err(HumanAuthorityFailure::GateBusy) if start.elapsed() < EPOCH_RETRY_DEADLINE => {
                    std::thread::sleep(EPOCH_RETRY_INTERVAL);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Complete the exact predecessor reap without holding the source epoch.
    fn reap_predecessor(&self, slot: &ConversationSlot) -> anyhow::Result<()> {
        slot.predecessor.reap(|previous| {
            let mut bound = previous.bound.lock().expect("predecessor conversation");
            bound.adapter.close_turn();
            bound.channel.close_and_reap()?;
            Ok(())
        })
    }

    /// Bind the exact resolved channel facts after the supervised admission,
    /// durably before the owned stdio channel is attached. A failed bind leaves
    /// no channel effect: the presented pipe is closed on drop. An explicit
    /// rebind (restart) advances the epoch durably and only then tears down the
    /// replaced attempt; a pinned message is never silently retargeted.
    /// An exact replay preserves a matching local slot. After a process
    /// restart the old pipe and correlation state cannot be reconstructed
    /// from the receipt; a new bind must advance the current epoch.
    pub fn admit_channel(
        &self,
        operation_id: Uuid,
        session_id: &str,
        expected_input_epoch: u64,
        channel: SessionConversationChannelV1,
        stdio: StdioConversation,
    ) -> anyhow::Result<SessionConversationBindingV1> {
        canonical_uuid(session_id)?;
        channel.validate()?;
        // Snapshot the currently published slot while holding the same epoch
        // used by bind/STOP publishers. Reap outside both publication guards.
        let observed = {
            let _epoch = self.write_epoch()?;
            self.sessions
                .lock()
                .expect("conversation registry")
                .get(session_id)
                .cloned()
        };
        if let Some(existing) = &observed {
            // A second rotation cannot replace this exact slot until its
            // predecessor is reaped. Same-bind replay follows this path too.
            self.reap_predecessor(existing)?;
        }

        // Exact replays reuse their already admitted child and native thread.
        // Resolve this before starting a thread on a newly presented pipe: a
        // replay after process loss must remain refused, not create an orphan.
        {
            let epoch = self.write_epoch()?;
            let access = HumanSourceAccess::write(&epoch, self.source)?;
            let sessions = self.sessions.lock().expect("conversation registry");
            let current_slot = sessions.get(session_id);
            let same_slot = match (&observed, current_slot) {
                (None, None) => true,
                (Some(observed), Some(current)) => Arc::ptr_eq(observed, current),
                _ => false,
            };
            anyhow::ensure!(same_slot, SessionMessageFailureV1::BindingChanged);
            let previous = self
                .store
                .session_conversation_binding_read(&access, session_id)?;
            anyhow::ensure!(
                previous.status != SessionConversationStatusV1::Closed,
                SessionMessageFailureV1::SessionClosed
            );
            let replay_candidate = previous.status == SessionConversationStatusV1::Active
                && expected_input_epoch.checked_add(1) == Some(previous.input_epoch)
                && channel_matches_binding(&channel, &previous);
            if replay_candidate {
                let binding = self.store.bind_session_conversation_scoped(
                    &epoch,
                    self.source,
                    &BindSessionConversationV1 {
                        version: 1,
                        operation_id,
                        session_id: session_id.into(),
                        expected_input_epoch,
                        channel: channel.clone(),
                    },
                )?;
                anyhow::ensure!(binding == previous, SessionMessageFailureV1::BindingChanged);
                let existing = current_slot
                    .cloned()
                    .ok_or(SessionMessageFailureV1::ChannelNotAdmitted)?;
                anyhow::ensure!(
                    existing.binding == binding,
                    SessionMessageFailureV1::BindingChanged
                );
                drop(sessions);
                drop(epoch);
                self.reap_predecessor(&existing)?;
                return Ok(binding);
            }
            anyhow::ensure!(
                previous.input_epoch == expected_input_epoch,
                SessionMessageFailureV1::BindingChanged
            );
        }

        // Codex app-server thread identity is accepted only from a correlated
        // response on this exact owned child. It is held process-locally and
        // never supplied by the channel DTO or an HTTP request.
        let thread_id = if channel.provider == "codex" {
            anyhow::ensure!(
                channel.protocol == "codex-app-server-v2",
                SessionMessageFailureV1::ChannelNotAdmitted
            );
            stdio
                .codex_thread_proof()
                .map_err(|_| SessionMessageFailureV1::ChannelNotAdmitted)?
                .thread_id()
                .to_owned()
        } else {
            session_id.to_owned()
        };

        let epoch = self.write_epoch()?;
        let access = HumanSourceAccess::write(&epoch, self.source)?;
        let mut sessions = self.sessions.lock().expect("conversation registry");
        let current_slot = sessions.get(session_id);
        let same_slot = match (&observed, current_slot) {
            (None, None) => true,
            (Some(observed), Some(current)) => Arc::ptr_eq(observed, current),
            _ => false,
        };
        anyhow::ensure!(same_slot, SessionMessageFailureV1::BindingChanged);
        let previous = self
            .store
            .session_conversation_binding_read(&access, session_id)?;
        let binding = self.store.bind_session_conversation_scoped(
            &epoch,
            self.source,
            &BindSessionConversationV1 {
                version: 1,
                operation_id,
                session_id: session_id.into(),
                expected_input_epoch,
                channel,
            },
        )?;
        let current = self
            .store
            .session_conversation_binding_read(&access, session_id)?;
        anyhow::ensure!(
            current.status != SessionConversationStatusV1::Closed,
            SessionMessageFailureV1::SessionClosed
        );
        anyhow::ensure!(
            current.status == SessionConversationStatusV1::Active && current == binding,
            SessionMessageFailureV1::BindingChanged
        );
        // A fresh bind advances the epoch. Only a replay leaves it unchanged.
        if previous == binding {
            let existing = current_slot
                .cloned()
                .ok_or(SessionMessageFailureV1::ChannelNotAdmitted)?;
            // The slot's identity is immutable. A replay need not wait for
            // dispatch, which may hold the channel while acquiring the epoch.
            anyhow::ensure!(
                existing.binding == binding,
                SessionMessageFailureV1::BindingChanged
            );
            drop(sessions);
            drop(epoch);
            self.reap_predecessor(&existing)?;
            return Ok(binding);
        }
        let fingerprint = binding.fingerprint()?;
        let slot = Arc::new(ConversationSlot {
            binding: binding.clone(),
            fingerprint,
            retiring: AtomicBool::new(false),
            bound: Mutex::new(BoundConversation {
                adapter: ConversationAdapterV1::new(),
                channel: stdio,
                thread_id,
            }),
            predecessor: PendingPredecessor::default(),
        });
        let replaced = current_slot.cloned();
        if let Some(previous) = &replaced {
            slot.predecessor.install(Arc::clone(previous))?;
        }
        sessions.insert(session_id.into(), Arc::clone(&slot));
        if let Some(replaced) = &replaced {
            // Publish retirement under the same epoch as the durable rebind.
            replaced.retiring.store(true, Ordering::Release);
        }
        // A replaced dispatch can need an epoch while it still owns the slot.
        drop(sessions);
        drop(epoch);
        if replaced.is_some() {
            // A failed reap leaves the newly installed slot explicitly gated;
            // replay/dispatch/STOP retain and retry this exact predecessor.
            self.reap_predecessor(&slot)?;
        }
        Ok(binding)
    }

    /// Drive the queued instructions of one session to the exact owned channel,
    /// one turn at a time. Each turn is latched durably before its first byte;
    /// only the observed complete write is transmitted and only the exact
    /// correlated typed response is acknowledged. A refusal, a cut or a timeout
    /// keeps the truthful durable status and never resends.
    pub fn dispatch(&self, session_id: &str) -> anyhow::Result<Vec<SessionMessageReceiptV1>> {
        canonical_uuid(session_id)?;
        let slot = self
            .sessions
            .lock()
            .expect("conversation registry")
            .get(session_id)
            .cloned();
        let Some(slot) = slot else {
            // No attached channel: distinguish the closed right to emit from a
            // binding that was never admitted, truthfully.
            let epoch = self.read_epoch()?;
            let access = HumanSourceAccess::read(&epoch, self.source)?;
            let binding = self
                .store
                .session_conversation_binding_read(&access, session_id)?;
            return Err(if binding.status == SessionConversationStatusV1::Closed {
                SessionMessageFailureV1::SessionClosed
            } else {
                SessionMessageFailureV1::ChannelNotAdmitted
            }
            .into());
        };
        // A newly rebound channel has no emission authority while its exact
        // predecessor is still owned but unreaped.
        self.reap_predecessor(&slot)?;
        let mut guard = slot.bound.lock().expect("admitted conversation");
        let bound = &mut *guard;
        let mut receipts = Vec::new();
        loop {
            let queued = {
                let epoch = self.read_epoch()?;
                let access = HumanSourceAccess::read(&epoch, self.source)?;
                self.store.session_message_queue(&access, session_id)?
            };
            let Some(operation_id) = queued.first().copied() else {
                break;
            };
            if bound.adapter.pending().is_some() {
                // A previous turn has no reconciled outcome yet: one turn at a
                // time, never interleaved.
                break;
            }
            // A known lost input has no possible effect. Leave the operation
            // queued instead of consuming its one durable latch.
            let mut input = bound.channel.prepare_input()?;
            // The durable latch before the first frame byte. The store
            // revalidates STOP, the original deadline, the resolved channel,
            // the exact pinned fingerprint and the retained initiator here.
            let emission = {
                let epoch = self.write_epoch()?;
                self.store.issue_session_message_emission_scoped(
                    &epoch,
                    self.source,
                    &IssueSessionMessageEmissionV1 {
                        version: 1,
                        operation_id,
                        expected_binding_fingerprint: slot.fingerprint.clone(),
                    },
                )?
            };
            anyhow::ensure!(
                emission.binding_fingerprint == slot.fingerprint
                    && emission.input_epoch == slot.binding.input_epoch,
                SessionMessageFailureV1::BindingChanged
            );
            let request =
                bound
                    .adapter
                    .issue_turn(operation_id, &bound.thread_id, &emission.content)?;
            let frame = request.frame()?;
            if let Err(error) = input.write_frame_guarded(
                &frame,
                |_written, deadline| -> anyhow::Result<_> {
                    #[cfg(test)]
                    self.pause_before_write_for_test(_written);
                    let epoch = self.write_epoch_until(deadline)?;
                    let expires_at = self.store.revalidate_session_message_emission_scoped(
                        &epoch,
                        self.source,
                        &emission,
                    )?;
                    Ok((epoch, expires_at))
                },
                |(epoch, expires_at)| {
                    epoch.check()?;
                    anyhow::ensure!(
                        chrono::Utc::now() < *expires_at,
                        SessionMessageFailureV1::BindingExpired
                    );
                    Ok(())
                },
            ) {
                // Possible emission with no observed complete write: delivery
                // stays unknown. Close input so no later frame can follow
                // a truncated one.
                bound.adapter.close_turn();
                bound.channel.close_input();
                return Err(error);
            }
            let mut receipt = {
                let epoch = self.write_epoch()?;
                self.store.record_session_message_transmitted_scoped(
                    &epoch,
                    self.source,
                    operation_id,
                )?
            };
            let mut acknowledged = false;
            let reconcile_until = Instant::now() + self.read_deadline;
            for _ in 0..MAX_RECONCILE_FRAMES {
                let line = match slot.next_response(&mut bound.channel, reconcile_until) {
                    Ok(line) => line,
                    // Retirement, EOF or timeout ends the wait without changing
                    // the last durably observed status.
                    Err(_) => break,
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if !value.is_object() {
                    continue;
                }
                if !acknowledged {
                    match bound.adapter.reconcile_response(&value) {
                        Some(AckObservationV1::Acknowledged { turn_id }) => {
                            let ack = SessionMessageAckV1 {
                                rpc_id: request.rpc_id,
                                turn_id,
                            };
                            #[cfg(test)]
                            {
                                let pause = self.before_ack.lock().unwrap().take();
                                if let Some(pause) = pause {
                                    pause();
                                }
                            }
                            let Some(epoch) = self.acknowledgement_epoch(&slot)? else {
                                break;
                            };
                            receipt = self.store.record_session_message_acknowledged_scoped(
                                &epoch,
                                self.source,
                                operation_id,
                                &ack,
                            )?;
                            acknowledged = true;
                        }
                        Some(AckObservationV1::Rejected) => {
                            // The provider refused the turn: transmitted, never
                            // acknowledged, never executed.
                            bound.adapter.close_turn();
                            break;
                        }
                        // Discordant id, malformed, unrelated frame or a
                        // duplicate: no new fact, keep reconciling.
                        _ => continue,
                    }
                } else {
                    match bound.adapter.reconcile_outcome(&value) {
                        Some(OutcomeObservationV1::Completed { .. }) => {
                            bound.adapter.close_turn();
                            break;
                        }
                        // Divergent provenance or unrelated notification: the
                        // turn result never advances on someone else's frame.
                        _ => continue,
                    }
                }
            }
            if bound.adapter.pending().is_some() {
                // No reconcilable outcome within the bound: free the channel.
                // The acknowledgement (if observed) remains the durable fact;
                // an unacknowledged emission keeps its uncertainty.
                bound.adapter.close_turn();
            }
            receipts.push(receipt);
            if slot.is_retiring() {
                break;
            }
        }
        Ok(receipts)
    }

    /// STOP priority: close the right to emit durably first, then close the
    /// exact owned channel. An audit failure leaves the channel untouched (no
    /// effect); the worker stop itself follows the broker's separate guard. A
    /// message never reopens the conversation, restarts the worker or resets
    /// its quota/time budget.
    pub fn stop(
        &self,
        operation_id: Uuid,
        session_id: &str,
    ) -> anyhow::Result<SessionConversationBindingV1> {
        canonical_uuid(session_id)?;
        let epoch = self.write_epoch()?;
        let binding = self.store.close_session_conversation_scoped(
            &epoch,
            self.source,
            &CloseSessionConversationV1 {
                version: 1,
                operation_id,
                session_id: session_id.into(),
            },
        )?;
        let slot = self
            .sessions
            .lock()
            .expect("conversation registry")
            .get(session_id)
            .cloned();
        if let Some(slot) = &slot {
            // Publish retirement under the same epoch as the durable close.
            slot.retiring.store(true, Ordering::Release);
        }
        // Dispatch may still need an epoch to observe closure while it owns
        // the slot. Wait for teardown only after releasing the epoch.
        drop(epoch);
        if let Some(slot) = slot {
            #[cfg(test)]
            if let Some(observer) = &self.stop_slot_observer {
                let _ = observer.send(());
            }
            let predecessor_result = self.reap_predecessor(&slot);
            let mut bound = slot.bound.lock().expect("admitted conversation");
            bound.adapter.close_turn();
            let current_result = bound.channel.close_and_reap();
            predecessor_result?;
            current_result?;
            drop(bound);
            // Durable Closed was published before teardown and prevents every
            // later bind. Remove only this exact slot; do not reacquire the
            // source epoch after ACK cancellation/reap just for local cleanup.
            let mut sessions = self.sessions.lock().expect("conversation registry");
            if sessions
                .get(session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                sessions.remove(session_id);
            }
        }
        Ok(binding)
    }
}
