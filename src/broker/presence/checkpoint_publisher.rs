//! Optional declared checkpoint publisher, sharing the existing presence
//! transport protections, permits and shutdown; it owns no runtime capability.
use super::*;
use crate::{audit::Store, broker_registry::*};
use std::time::Duration;

#[cfg(test)]
#[path = "schema13_checkpoint_tests.rs"]
mod schema13_checkpoint_tests;

fn stream(p: &PreparedPresence) -> Result<&checkpoint_storage::Stream> {
    p.checkpoint_stream
        .as_ref()
        .ok_or(PresenceError::UnsafeState)
}

/// Only a retained Pending may bridge the explicit semantic 12-to-13 upgrade.
/// Current identity came through the authenticated gateway handshake. No field
/// is rewritten, and this is not a general identity-equivalence rule.
fn pending_identity_matches_current(
    pending: &BrokerIdentityViewV1,
    current: &BrokerIdentityViewV1,
) -> bool {
    matches!(
        (pending.schema_version, current.schema_version),
        (12, 12) | (13, 13) | (12, 13)
    ) && pending.version == 1
        && current.version == 1
        && pending.tenant_id == current.tenant_id
        && pending.control_namespace_uuid == current.control_namespace_uuid
        && pending.broker_id == current.broker_id
        && pending.incarnation_id == current.incarnation_id
        && pending.credential_id == current.credential_id
        && pending.binding_revision == current.binding_revision
        && pending.grant_revision == current.grant_revision
}
fn validate_remote(
    p: &PreparedPresence,
    identity: &BrokerIdentityViewV1,
    view: &BrokerCheckpointViewV1,
    head: &checkpoint_storage::Head,
) -> Result<()> {
    actor::validate_handshake(p, identity, &view.broker)?;
    if !matches!(identity.schema_version, 12 | 13) || view.version != 1 {
        return Err(PresenceError::Forbidden);
    }
    if let Some(remote) = &view.checkpoint {
        remote
            .checkpoint
            .validate()
            .map_err(|_| PresenceError::Protocol)?;
        if remote.publication_provenance != BrokerClaimProvenanceV1::Declared
            || remote.reception_provenance != BrokerReceptionProvenanceV1::Observed
            || remote.coverage != BrokerCheckpointCoverageV1::DeclaredPartial
        {
            return Err(PresenceError::Protocol);
        }
        if remote.freshness == BrokerCheckpointFreshnessV1::ClockUncertain {
            return Err(PresenceError::Clock);
        }
    }
    if let Some(pending) = &head.pending {
        if !pending_identity_matches_current(&pending.identity, identity) {
            return Err(PresenceError::Conflict);
        }
        // A committed response may have been lost. It must be the exact
        // retained operation; the replay never allocates a new sequence.
        if let Some(remote) = &view.checkpoint
            && remote.in_current_grant
            && remote.checkpoint == pending.checkpoint
        {
            let result = BrokerCheckpointResultV1 {
                checkpoint: remote.checkpoint.clone(),
                receipt: remote.receipt.clone(),
                replayed: true,
            };
            checkpoint_storage::validate_result(pending, &result)?;
            return Ok(());
        }
    }
    if let Some((last, result)) = &head.last {
        let remote = view.checkpoint.as_ref().ok_or(PresenceError::Conflict)?;
        if !remote.in_current_grant
            || remote.checkpoint != result.checkpoint
            || remote.receipt != result.receipt
            || identity.grant_revision != last.identity.grant_revision
            || identity.binding_revision < last.identity.binding_revision
        {
            return Err(PresenceError::Conflict);
        }
    } else if view
        .checkpoint
        .as_ref()
        .is_some_and(|remote| remote.checkpoint.incarnation_id == identity.incarnation_id)
    {
        // Missing local history cannot be adopted from a remote serialized
        // checkpoint, even when its scope appears plausible.
        return Err(PresenceError::Conflict);
    }
    Ok(())
}

pub(super) async fn run(
    p: Arc<PreparedPresence>,
    mut snapshots: watch::Receiver<Option<BrokerCheckpointSnapshotV1>>,
    shutdown: &mut watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<()> {
    let client = http::Client::new(&p)?;
    let mut binding_this_run = None;
    let mut next = snapshots.borrow_and_update().clone();
    let mut source_ready = true;
    loop {
        if *shutdown.borrow() {
            return Err(PresenceError::Stopped);
        }
        let head = actor::disk(&p, &io, shutdown, |p| stream(p)?.checked_head(p)).await?;
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
            }
            source_ready = false;
            let Some(snapshot) = &next else {
                continue;
            };
            snapshot.validate().map_err(|_| PresenceError::Protocol)?;
            if snapshot.authority.authority_id != p.config.authority_id
                || snapshot.authority.runtime_kind != p.scope.runtime_kind
            {
                return Err(PresenceError::Forbidden);
            }
        }
        let mut backoff = 0usize;
        loop {
            let permit = actor::interrupt(shutdown, async {
                Arc::clone(&attempts)
                    .acquire_owned()
                    .await
                    .map_err(|_| PresenceError::Unavailable)
            })
            .await?;
            let attempt = async {
                let head = actor::disk(&p, &io, shutdown, |p| stream(p)?.checked_head(p)).await?;
                let identity = actor::interrupt(shutdown, client.identity(&p)).await?;
                let view = actor::interrupt(shutdown, client.checkpoint_view(&p)).await?;
                validate_remote(&p, &identity, &view, &head)?;
                if binding_this_run.is_some_and(|binding| binding != identity.binding_revision) {
                    return Err(PresenceError::Conflict);
                }
                binding_this_run = Some(identity.binding_revision);
                if head.pending.is_none() {
                    let snapshot = next.clone().ok_or(PresenceError::Unavailable)?;
                    let previous_sequence = head
                        .last
                        .as_ref()
                        .map_or(0, |(last, _)| last.checkpoint.sequence);
                    let checkpoint = BrokerCheckpointV1 {
                        version: 1,
                        operation_id: Uuid::new_v4().to_string(),
                        incarnation_id: identity.incarnation_id.clone(),
                        previous_sequence,
                        sequence: previous_sequence
                            .checked_add(1)
                            .ok_or(PresenceError::Capacity)?,
                        snapshot,
                    };
                    let input_hash = Store::broker_checkpoint_input_hash(&identity, &checkpoint)
                        .map_err(|_| PresenceError::Protocol)?;
                    let pending = checkpoint_storage::Pending {
                        version: 1,
                        identity,
                        checkpoint,
                        input_hash,
                    };
                    actor::disk(&p, &io, shutdown, move |p| stream(p)?.append(p, pending)).await?;
                }
                let pending = actor::disk(&p, &io, shutdown, |p| {
                    stream(p)?.pending(p)?.ok_or(PresenceError::UnsafeState)
                })
                .await?;
                let result =
                    actor::interrupt(shutdown, client.checkpoint_post(&p, &pending)).await?;
                checkpoint_storage::validate_result(&pending, &result)?;
                actor::disk(&p, &io, shutdown, move |p| {
                    stream(p)?.acknowledge(p, result)
                })
                .await
            }
            .await;
            drop(permit);
            match attempt {
                Ok(()) => break,
                Err(PresenceError::Transport | PresenceError::Unavailable) => {
                    let delay = if backoff < 3 {
                        let millis = [100, 250, 500][backoff];
                        backoff += 1;
                        Duration::from_millis(millis)
                    } else {
                        Duration::from_secs(u64::from(p.config.interval_seconds.min(30)))
                    };
                    actor::interrupt(shutdown, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
                Err(error) => return Err(error),
            }
        }
        // Changes during an unavailable/unknown response never replace the
        // durable Pending. After its ACK, coalesce to the newest source value.
        // The original broker journal remains the full history; this stream
        // only declares sampled heads and cannot claim complete observation.
        if *shutdown.borrow() {
            return Err(PresenceError::Stopped);
        }
        if snapshots
            .has_changed()
            .map_err(|_| PresenceError::Unavailable)?
        {
            next = snapshots.borrow_and_update().clone();
            source_ready = true;
        }
    }
}
