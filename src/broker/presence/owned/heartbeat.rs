//! Owned heartbeat actor. One durable Pending at a time; it is acknowledged
//! only as the exact successor of its retained prior view, or by an exact
//! self-read of the committed operation after a lost response. Retries,
//! timeouts and restarts resend the same durable bytes and operation ID.
use super::journal::{self, AckRecordV2, Head, Kind};
use super::*;
use super::{schedule, transport};
use crate::broker::presence::actor;
use crate::broker_registry::{
    BrokerV1,
    owned_supervision::{
        MAX_OWNED_HEARTBEAT_PENDING_BYTES, MAX_OWNED_HEARTBEAT_RESULT_BYTES,
        OwnedAcknowledgementSourceV2, OwnedBrokerHeartbeatResultV2, OwnedBrokerSelfViewV2,
        OwnedHeartbeatPendingV2, check_progress_identity, check_replay_identity,
        validate_heartbeat_handshake,
    },
};
use uuid::Uuid;
use crate::broker_registry::owned_supervision::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct HeartbeatJournal;

impl Kind for HeartbeatJournal {
    fn pending_identity(pending: &Self::Pending) -> &OwnedBrokerIdentityViewV2 { &pending.identity }
    fn outcome_identity(outcome: &Self::Outcome) -> &OwnedBrokerIdentityViewV2 { &outcome.identity }
    fn operation_id(pending: &Self::Pending) -> &str { &pending.heartbeat.operation_id }
    fn recovery_request(pending: &Self::Pending, previous: Option<&AckRecordV2<Self::Outcome>>) -> OwnedReconciliationRequestV2 {
        OwnedReconciliationRequestV2::Heartbeat { pending: Box::new(pending.clone()), previous: previous.map(|last| Box::new(last.result.clone())) }
    }
    fn recovered(proof: &OwnedReconciliationResultV2) -> Result<Option<Self::Outcome>> {
        match &proof.state {
            OwnedReconciliationStateV2::CommittedExact { historical: OwnedHistoricalResultV2::Heartbeat { result } } => Ok(Some((**result).clone())),
            OwnedReconciliationStateV2::NonCommitted => Ok(None),
            _ => Err(PresenceError::Protocol),
        }
    }
    fn replacement(pending: &Self::Pending, proof: &OwnedReconciliationResultV2, operation_id: String) -> Result<Self::Pending> {
        let OwnedReconciliationHeadV2::Heartbeat { view, receipt } = &proof.head else { return Err(PresenceError::Protocol); };
        let view = OwnedBrokerSelfViewV2::new(proof.observation.clone(), view.clone(), receipt.clone()).map_err(contract)?;
        let replacement = OwnedHeartbeatPendingV2::prepare(&view, operation_id).map_err(contract)?;
        if replacement.heartbeat.operation_id == pending.heartbeat.operation_id
            || replacement.heartbeat.sequence != pending.heartbeat.sequence
            || replacement.heartbeat.expected_heartbeat_revision != pending.heartbeat.expected_heartbeat_revision
            || replacement.prior.broker != pending.prior.broker {
            return Err(PresenceError::UnsafeState);
        }
        Ok(replacement)
    }
    const NAME: &'static str = "heartbeat";
    const RECORD_BYTES: u64 = journal::record_bytes(
        MAX_OWNED_HEARTBEAT_PENDING_BYTES,
        MAX_OWNED_HEARTBEAT_RESULT_BYTES,
    );
    type Pending = OwnedHeartbeatPendingV2;
    type Outcome = OwnedBrokerHeartbeatResultV2;

    fn check_pending(
        pins: &OwnedSupervisionPinsV2,
        previous: Option<&AckRecordV2<Self::Outcome>>,
        pending: &Self::Pending,
    ) -> Result<()> {
        pending.validate().map_err(contract)?;
        pins.check_identity(&pending.identity).map_err(identity)?;
        match previous {
            Some(last) => {
                check_progress_identity(&last.observation, &pending.identity)
                    .map_err(identity)?;
                if pending.prior.broker != last.result.result.broker {
                    return Err(block(OwnedPresenceBlock::RemoteHeadDiverged));
                }
            }
            None if has_remote_head(
                &pending.prior.broker,
                &pending.identity.subject.incarnation_id,
            ) =>
            {
                return Err(block(OwnedPresenceBlock::RemoteHeadWithoutHistory));
            }
            None => {}
        }
        Ok(())
    }

    fn check_outcome(
        pins: &OwnedSupervisionPinsV2,
        pending: &Self::Pending,
        source: OwnedAcknowledgementSourceV2,
        outcome: &Self::Outcome,
    ) -> Result<()> {
        pending.validate_result(outcome).map_err(acknowledgement)?;
        pins.check_identity(&outcome.identity).map_err(identity)?;
        // A self-read acknowledgement always reports the original receipt.
        if source == OwnedAcknowledgementSourceV2::SelfRead && !outcome.result.replayed {
            return Err(acknowledgement(OwnedAckRejectionV2::Protocol));
        }
        Ok(())
    }

    fn audit_seq(outcome: &Self::Outcome) -> i64 {
        outcome.result.receipt.audit_seq
    }
}

fn has_remote_head(broker: &BrokerV1, incarnation_id: &str) -> bool {
    broker
        .last_heartbeat
        .as_ref()
        .is_some_and(|heartbeat| heartbeat.incarnation_id == incarnation_id)
}

/// The control head must still be this journal's last acknowledged state, or
/// carry no heartbeat of this incarnation for a journal without history.
fn check_continuity(head: &Head<HeartbeatJournal>, view: &OwnedBrokerSelfViewV2) -> Result<()> {
    match &head.last {
        Some(last) if view.view.broker != last.result.result.broker => {
            Err(block(OwnedPresenceBlock::RemoteHeadDiverged))
        }
        None if has_remote_head(&view.view.broker, &view.identity.subject.incarnation_id) => {
            Err(block(OwnedPresenceBlock::RemoteHeadWithoutHistory))
        }
        Some(_) | None => Ok(()),
    }
}

pub(super) async fn run(
    p: Arc<PreparedOwnedPresence>,
    shutdown: &mut watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<()> {
    let client = transport::OwnedClient::new(&p)?;
    loop {
        let mut retry = schedule::Retry::new(HeartbeatJournal::NAME);
        loop {
            let permit = schedule::permit(shutdown, &attempts).await?;
            let outcome = attempt(&p, &client, &io, shutdown).await;
            drop(permit);
            match outcome {
                Ok(()) => {
                    retry.resolved(&p);
                    break;
                }
                // Same retained operation, same client and journal.
                Err(error) => retry.after(&p, shutdown, error).await?,
            }
        }
        schedule::pause(shutdown, p.publication_interval()).await?;
    }
}

async fn attempt(
    p: &Arc<PreparedOwnedPresence>,
    client: &transport::OwnedClient,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    let mut head = schedule::disk(p, io, shutdown, |p| p.heartbeat_journal()?.cached_head()).await?;
    if let Some(replacement) = head.replacement.clone() {
        // A durable closure already fixed these exact bytes and UUID.
        schedule::disk(p, io, shutdown, move |p| p.heartbeat_journal()?.append(p, replacement)).await?;
        head = schedule::disk(p, io, shutdown, |p| p.heartbeat_journal()?.cached_head()).await?;
    }
    let view = actor::interrupt(shutdown, client.self_view(p)).await?;
    // Any refusal below leaves the anchor and the Pending untouched.
    validate_heartbeat_handshake(&p.pins, &view, p.min_lease()).map_err(handshake)?;
    schedule::observe(p, io, shutdown, &view.identity).await?;
    // Either way, the POST below sends the durable bytes of the retained
    // Pending, decoded right after a complete verification of the journal.
    let pending = if let Some(pending) = &head.pending {
        if pending.identity.subject.binding_revision != view.identity.subject.binding_revision {
            check_progress_identity(&pending.identity, &view.identity).map_err(identity)?;
            let request = HeartbeatJournal::recovery_request(pending, head.last.as_ref());
            let proof = actor::interrupt(shutdown, client.reconcile(p, &request)).await?;
            proof.validate_for(&request).map_err(contract)?;
            schedule::observe(p, io, shutdown, &proof.observation).await?;
            if HeartbeatJournal::recovered(&proof)?.is_some() {
                return schedule::disk(p, io, shutdown, move |p| p.heartbeat_journal()?.acknowledge_reconciled(p, proof)).await;
            }
            let expected = pending.clone();
            let replacement = HeartbeatJournal::replacement(pending, &proof, Uuid::new_v4().to_string())?;
            return schedule::disk(p, io, shutdown, move |p| p.heartbeat_journal()?.supersede(p, &expected, proof, replacement)).await;
        }
        // Restart, timeout or lost ACK: replay only under the same subject.
        check_replay_identity(&pending.identity, &view.identity).map_err(identity)?;
        if let Some(result) = pending.recognize(&view).map_err(acknowledgement)? {
            return acknowledge(p, io, shutdown, OwnedAcknowledgementSourceV2::SelfRead, result)
                .await;
        }
        check_continuity(&head, &view)?;
        schedule::disk(p, io, shutdown, |p| {
            p.heartbeat_journal()?.durable_pending(p)
        })
        .await?
    } else {
        if let Some(last) = &head.last {
            check_progress_identity(&last.observation, &view.identity).map_err(identity)?;
        }
        check_continuity(&head, &view)?;
        let pending = OwnedHeartbeatPendingV2::prepare(&view, Uuid::new_v4().to_string())
            .map_err(contract)?;
        schedule::disk(p, io, shutdown, move |p| {
            p.heartbeat_journal()?.append(p, pending)
        })
        .await?
    };
    let result = actor::interrupt(shutdown, client.heartbeat(p, &pending)).await?;
    // A 200 is not enough: the result must be the exact expected successor.
    pending.validate_result(&result).map_err(acknowledgement)?;
    acknowledge(p, io, shutdown, OwnedAcknowledgementSourceV2::Response, result).await
}

async fn acknowledge(
    p: &Arc<PreparedOwnedPresence>,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
    source: OwnedAcknowledgementSourceV2,
    result: OwnedBrokerHeartbeatResultV2,
) -> Result<()> {
    schedule::disk(p, io, shutdown, move |p| {
        p.heartbeat_journal()?.acknowledge(p, source, result)
    })
    .await
}
