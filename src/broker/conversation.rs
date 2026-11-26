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
}

/// The immutable durable binding and the signal to retire its owned channel.
struct ConversationSlot {
    binding: SessionConversationBindingV1,
    fingerprint: String,
    retiring: AtomicBool,
    bound: Mutex<BoundConversation>,
}

impl ConversationSlot {
    fn next_response(
        &self,
        channel: &mut StdioConversation,
        deadline: Instant,
    ) -> io::Result<String> {
        loop {
            if self.retiring.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match channel.read_line(remaining.min(READ_RETIREMENT_CHECK_INTERVAL)) {
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
    stop_slot_observer: Option<std::sync::mpsc::Sender<()>>,
}

impl<'a> SessionConversationComposerV1<'a> {
    /// Compose over one retained owned source. The read deadline bounds each
    /// response observation; it never extends the binding's original expiry.
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
    pub(crate) fn observe_stop_slot_for_test(&mut self, observer: std::sync::mpsc::Sender<()>) {
        self.stop_slot_observer = Some(observer);
    }

    /// Write epoch with bounded transient-busy retry (STOP keeps priority over
    /// the business drain by closing the right to emit at the effect point).
    fn write_epoch(&self) -> anyhow::Result<HumanSourcesWriteEpoch> {
        self.write_epoch_until(Instant::now() + EPOCH_RETRY_DEADLINE)
    }

    fn write_epoch_until(&self, deadline: Instant) -> anyhow::Result<HumanSourcesWriteEpoch> {
        loop {
            match self.owner.try_write_epoch() {
                Ok(epoch) => return Ok(epoch),
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
        let epoch = self.write_epoch()?;
        let access = HumanSourceAccess::write(&epoch, self.source)?;
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
            let existing = self
                .sessions
                .lock()
                .expect("conversation registry")
                .get(session_id)
                .cloned()
                .ok_or(SessionMessageFailureV1::ChannelNotAdmitted)?;
            // The slot's identity is immutable. A replay need not wait for
            // dispatch, which may hold the channel while acquiring the epoch.
            anyhow::ensure!(
                existing.binding == binding,
                SessionMessageFailureV1::BindingChanged
            );
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
            }),
        });
        let replaced = self
            .sessions
            .lock()
            .expect("conversation registry")
            .insert(session_id.into(), slot);
        // A replaced dispatch can need an epoch while it still owns the slot.
        drop(epoch);
        if let Some(replaced) = replaced {
            // The durable bind committed: the replaced attempt's channel is
            // torn down explicitly, never by a message.
            replaced.retiring.store(true, Ordering::Release);
            let mut old = replaced.bound.lock().expect("replaced conversation");
            old.adapter.close_turn();
            old.channel.close();
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
            let request = bound
                .adapter
                .issue_turn(operation_id, session_id, &emission.content)?;
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
                |(_, expires_at)| {
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
                            let epoch = self.write_epoch()?;
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
            if slot.retiring.load(Ordering::Acquire) {
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
        // The durable close already fences writing. Dispatch must be able to
        // read that close under its own epoch while it still owns the slot.
        drop(epoch);
        let slot = self
            .sessions
            .lock()
            .expect("conversation registry")
            .get(session_id)
            .cloned();
        if let Some(slot) = slot {
            // The durable close succeeded. Interrupt any in-progress
            // reconciliation before waiting to reap its exact channel.
            slot.retiring.store(true, Ordering::Release);
            #[cfg(test)]
            if let Some(observer) = &self.stop_slot_observer {
                let _ = observer.send(());
            }
            let mut bound = slot.bound.lock().expect("admitted conversation");
            bound.adapter.close_turn();
            bound.channel.close_and_reap()?;
            // A replay must find this slot until teardown has completed.
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
