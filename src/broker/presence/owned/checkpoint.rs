//! Owned checkpoint actor. It declares only heads already acknowledged by an
//! admitted Full writer, sampled by the service; an absent source (cold,
//! fenced or incomplete service) declares nothing and is never an empty
//! inventory. One durable Pending at a time: a newer source value is
//! coalesced only after the acknowledgement is durable, and a remote head is
//! never adopted without this journal's acknowledged history.
use super::journal::{self, AckRecordV1, Head, Kind};
use super::*;
use super::{schedule, transport};
use crate::broker::presence::actor;
use crate::broker_registry::owned_supervision::{
    MAX_OWNED_CHECKPOINT_PENDING_BYTES, MAX_OWNED_CHECKPOINT_RESULT_BYTES,
    OwnedAcknowledgementSourceV1, OwnedBrokerCheckpointResultV1, OwnedBrokerCheckpointSelfViewV1,
    OwnedCheckpointPendingV1, check_progress_identity, check_replay_identity,
    validate_checkpoint_handshake,
};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CheckpointJournal;

impl Kind for CheckpointJournal {
    const NAME: &'static str = "checkpoint";
    const RECORD_BYTES: u64 = journal::record_bytes(
        MAX_OWNED_CHECKPOINT_PENDING_BYTES,
        MAX_OWNED_CHECKPOINT_RESULT_BYTES,
    );
    type Pending = OwnedCheckpointPendingV1;
    type Outcome = OwnedBrokerCheckpointResultV1;

    fn check_pending(
        pins: &OwnedSupervisionPinsV1,
        previous: Option<&AckRecordV1<Self::Outcome>>,
        pending: &Self::Pending,
    ) -> Result<()> {
        pending.validate().map_err(contract)?;
        pins.check_identity(&pending.identity).map_err(identity)?;
        pins.check_snapshot_authority(&pending.checkpoint.snapshot)
            .map_err(identity)?;
        let previous_sequence =
            previous.map_or(0, |last| last.result.result.checkpoint.sequence);
        if pending.checkpoint.previous_sequence != previous_sequence {
            return Err(contract(OwnedSupervisionErrorCodeV1::SequenceConflict));
        }
        if let Some(last) = previous {
            check_progress_identity(&last.result.identity, &pending.identity)
                .map_err(identity)?;
        }
        Ok(())
    }

    fn check_outcome(
        pins: &OwnedSupervisionPinsV1,
        pending: &Self::Pending,
        source: OwnedAcknowledgementSourceV1,
        outcome: &Self::Outcome,
    ) -> Result<()> {
        pending.validate_result(outcome).map_err(acknowledgement)?;
        pins.check_identity(&outcome.identity).map_err(identity)?;
        // A self-read acknowledgement always reports the original receipt.
        if source == OwnedAcknowledgementSourceV1::SelfRead && !outcome.result.replayed {
            return Err(acknowledgement(OwnedAckRejectionV1::Protocol));
        }
        Ok(())
    }

    fn audit_seq(outcome: &Self::Outcome) -> i64 {
        outcome.result.receipt.audit_seq
    }
}

/// The control head must still be this journal's last acknowledgement (same
/// checkpoint and receipt, in the current grant), or carry no checkpoint of
/// this incarnation for a journal without history. Returns the local head.
fn check_continuity(
    head: &Head<CheckpointJournal>,
    view: &OwnedBrokerCheckpointSelfViewV1,
) -> Result<u64> {
    let remote = view.view.checkpoint.as_ref();
    match &head.last {
        Some(last) => {
            let acknowledged = &last.result.result;
            if remote.is_some_and(|observed| {
                observed.in_current_grant
                    && observed.checkpoint == acknowledged.checkpoint
                    && observed.receipt == acknowledged.receipt
            }) {
                Ok(acknowledged.checkpoint.sequence)
            } else {
                Err(block(OwnedPresenceBlock::RemoteHeadDiverged))
            }
        }
        None if remote.is_some_and(|observed| {
            observed.checkpoint.incarnation_id == view.identity.subject.incarnation_id
        }) =>
        {
            Err(block(OwnedPresenceBlock::RemoteHeadWithoutHistory))
        }
        None => Ok(0),
    }
}

pub(super) async fn run(
    p: Arc<PreparedOwnedPresence>,
    mut snapshots: watch::Receiver<Option<BrokerCheckpointSnapshotV1>>,
    shutdown: &mut watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<()> {
    let client = transport::OwnedClient::new(&p)?;
    let mut next = snapshots.borrow_and_update().clone();
    let mut source_ready = true;
    // The initial value precedes the first sample; only a sampled absence
    // is reported, and only on a transition.
    let mut sampled = false;
    let mut absent = false;
    loop {
        if *shutdown.borrow() {
            return Err(PresenceError::Stopped);
        }
        let head =
            schedule::disk(&p, &io, shutdown, |p| p.checkpoint_journal()?.cached_head()).await?;
        if head.pending.is_none() {
            if !source_ready {
                actor::interrupt(shutdown, async {
                    snapshots
                        .changed()
                        .await
                        .map_err(|_| PresenceError::Unavailable)
                })
                .await?;
                next = snapshots.borrow_and_update().clone();
                sampled = true;
            }
            source_ready = false;
            let Some(snapshot) = &next else {
                if sampled && !absent {
                    absent = true;
                    p.report(
                        CheckpointJournal::NAME,
                        "checkpoint_source_unavailable",
                        None,
                        None,
                    );
                }
                continue;
            };
            if std::mem::take(&mut absent) {
                p.report(
                    CheckpointJournal::NAME,
                    "checkpoint_source_available",
                    None,
                    None,
                );
            }
            snapshot.validate().map_err(|_| PresenceError::Protocol)?;
            p.pins
                .check_snapshot_authority(snapshot)
                .map_err(identity)?;
        }
        let mut retry = schedule::Retry::new(CheckpointJournal::NAME);
        loop {
            let permit = schedule::permit(shutdown, &attempts).await?;
            let outcome = attempt(&p, &client, &io, shutdown, next.as_ref()).await;
            drop(permit);
            match outcome {
                Ok(()) => {
                    retry.resolved(&p);
                    break;
                }
                // Source changes never replace the retained operation.
                Err(error) => retry.after(&p, shutdown, error).await?,
            }
        }
        if *shutdown.borrow() {
            return Err(PresenceError::Stopped);
        }
        // After the durable ACK only, coalesce to the newest source value.
        if snapshots
            .has_changed()
            .map_err(|_| PresenceError::Unavailable)?
        {
            next = snapshots.borrow_and_update().clone();
            source_ready = true;
            sampled = true;
        }
    }
}

async fn attempt(
    p: &Arc<PreparedOwnedPresence>,
    client: &transport::OwnedClient,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
    next: Option<&BrokerCheckpointSnapshotV1>,
) -> Result<()> {
    let head = schedule::disk(p, io, shutdown, |p| p.checkpoint_journal()?.cached_head()).await?;
    let view = actor::interrupt(shutdown, client.checkpoint_self(p)).await?;
    // Any refusal below leaves the anchor and the Pending untouched.
    validate_checkpoint_handshake(&p.pins, &view, p.min_lease()).map_err(handshake)?;
    p.observe_closure(&view.identity)?;
    // Either way, the POST below sends the durable bytes of the retained
    // Pending, decoded right after a complete verification of the journal.
    let pending = if let Some(pending) = &head.pending {
        // Restart, timeout or lost ACK: replay only under the same subject.
        check_replay_identity(&pending.identity, &view.identity).map_err(identity)?;
        if let Some(result) = pending.recognize(&view).map_err(acknowledgement)? {
            return acknowledge(p, io, shutdown, OwnedAcknowledgementSourceV1::SelfRead, result)
                .await;
        }
        check_continuity(&head, &view)?;
        schedule::disk(p, io, shutdown, |p| {
            p.checkpoint_journal()?.durable_pending(p)
        })
        .await?
    } else {
        if let Some(last) = &head.last {
            check_progress_identity(&last.result.identity, &view.identity).map_err(identity)?;
        }
        // The previous sequence is the local acknowledged head, never a
        // remote value without this journal's history.
        let previous_sequence = check_continuity(&head, &view)?;
        let snapshot = next.cloned().ok_or(PresenceError::Unavailable)?;
        let pending = OwnedCheckpointPendingV1::prepare(
            &view.identity,
            previous_sequence,
            snapshot,
            Uuid::new_v4().to_string(),
        )
        .map_err(contract)?;
        schedule::disk(p, io, shutdown, move |p| {
            p.checkpoint_journal()?.append(p, pending)
        })
        .await?
    };
    let result = actor::interrupt(shutdown, client.checkpoint(p, &pending)).await?;
    // A 200 is not enough: the result must carry exactly this checkpoint.
    pending.validate_result(&result).map_err(acknowledgement)?;
    acknowledge(p, io, shutdown, OwnedAcknowledgementSourceV1::Response, result).await
}

async fn acknowledge(
    p: &Arc<PreparedOwnedPresence>,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
    source: OwnedAcknowledgementSourceV1,
    result: OwnedBrokerCheckpointResultV1,
) -> Result<()> {
    schedule::disk(p, io, shutdown, move |p| {
        p.checkpoint_journal()?.acknowledge(p, source, result)
    })
    .await
}
