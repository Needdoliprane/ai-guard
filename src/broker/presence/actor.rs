use super::*;
use crate::{audit::Store, broker_registry::*};
use std::{future::Future, time::Duration};

async fn stopping(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}
async fn interrupt<T>(
    shutdown: &mut watch::Receiver<bool>,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! { biased;
        _ = stopping(shutdown) => Err(PresenceError::Stopped),
        result = future => result,
    }
}
async fn disk<T: Send + 'static>(
    p: &Arc<PreparedPresence>,
    io: &Arc<Semaphore>,
    shutdown: &mut watch::Receiver<bool>,
    action: impl FnOnce(&PreparedPresence) -> Result<T> + Send + 'static,
) -> Result<T> {
    let capacity = interrupt(shutdown, async {
        Arc::clone(io)
            .acquire_owned()
            .await
            .map_err(|_| PresenceError::Unavailable)
    })
    .await?;
    let p = Arc::clone(p);
    // A cancelled actor may leave this bounded filesystem closure finishing.
    // It has no HTTP client and can never issue a request after actor shutdown.
    let task = tokio::task::spawn_blocking(move || {
        let _capacity = capacity;
        p.revalidate()?;
        action(&p)
    });
    interrupt(shutdown, async {
        task.await.map_err(|_| PresenceError::Unavailable)?
    })
    .await
}
fn stream(p: &PreparedPresence) -> Result<&storage::Stream> {
    p.stream.as_ref().ok_or(PresenceError::UnsafeState)
}

pub(super) async fn run(
    p: PreparedPresence,
    mut shutdown: watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<()> {
    let result = run_owned(Arc::new(p), &mut shutdown, attempts, io).await;
    match result {
        Err(PresenceError::Stopped) => Ok(()),
        result => result,
    }
}
async fn run_owned(
    p: Arc<PreparedPresence>,
    shutdown: &mut watch::Receiver<bool>,
    attempts: Arc<Semaphore>,
    io: Arc<Semaphore>,
) -> Result<()> {
    if p.stream.is_none() {
        return Err(PresenceError::UnsafeState);
    }
    let client = http::Client::new(&p)?;
    let mut binding_this_run: Option<u64> = None;
    loop {
        let mut acknowledged = false;
        for retry in 0..=3u32 {
            let permit = interrupt(shutdown, async {
                Arc::clone(&attempts)
                    .acquire_owned()
                    .await
                    .map_err(|_| PresenceError::Unavailable)
            })
            .await?;
            let attempt = async {
                let head = disk(&p, &io, shutdown, |p| stream(p)?.cached_head()).await?;
                let identity = interrupt(shutdown, client.identity(&p)).await?;
                let view = interrupt(shutdown, client.view(&p)).await?;
                validate_handshake(&p, &identity, &view)?;
                if let Some(binding) = binding_this_run {
                    if binding != identity.binding_revision {
                        return Err(PresenceError::Conflict);
                    }
                }
                if let Some(pending) = &head.pending {
                    if identity != pending.identity
                        || !same_grant(&view.broker, &pending.prior.broker)
                    {
                        return Err(PresenceError::Conflict);
                    }
                } else if let Some(last) = &head.last {
                    if view.broker != last.result.broker
                        || identity.grant_revision != last.pending.identity.grant_revision
                        || identity.binding_revision < last.pending.identity.binding_revision
                    {
                        return Err(PresenceError::Conflict);
                    }
                } else if view
                    .broker
                    .last_heartbeat
                    .as_ref()
                    .is_some_and(|h| h.incarnation_id == identity.incarnation_id)
                {
                    return Err(PresenceError::Conflict);
                }
                binding_this_run = Some(identity.binding_revision);
                if head.pending.is_none() {
                    let previous = view
                        .broker
                        .last_heartbeat
                        .as_ref()
                        .filter(|h| h.incarnation_id == identity.incarnation_id)
                        .map_or(0, |h| h.sequence);
                    let heartbeat = BrokerHeartbeatV1 {
                        version: 1,
                        operation_id: Uuid::new_v4().to_string(),
                        expected_heartbeat_revision: view.broker.heartbeat_revision,
                        incarnation_id: identity.incarnation_id.clone(),
                        sequence: previous.checked_add(1).ok_or(PresenceError::Capacity)?,
                    };
                    let input_hash = Store::broker_heartbeat_input_hash(&identity, &heartbeat)
                        .map_err(|_| PresenceError::Protocol)?;
                    let pending = storage::Pending {
                        version: 1,
                        identity,
                        prior: view,
                        heartbeat,
                        input_hash,
                    };
                    disk(&p, &io, shutdown, move |p| stream(p)?.append(p, pending)).await?;
                }
                let pending = disk(&p, &io, shutdown, |p| stream(p)?.revalidate_pending(p)).await?;
                let result = interrupt(shutdown, client.post(&p, &pending)).await?;
                storage::validate_result(&pending, &result)?;
                disk(&p, &io, shutdown, move |p| {
                    stream(p)?.acknowledge(p, result)
                })
                .await
            }
            .await;
            drop(permit);
            match attempt {
                Ok(()) => {
                    acknowledged = true;
                    break;
                }
                Err(error @ (PresenceError::Transport | PresenceError::Unavailable))
                    if retry < 3 =>
                {
                    let millis = [100, 250, 500][retry as usize];
                    interrupt(shutdown, async {
                        tokio::time::sleep(Duration::from_millis(millis)).await;
                        Ok(())
                    })
                    .await?;
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        if !acknowledged {
            return Err(PresenceError::Transport);
        }
        interrupt(shutdown, async {
            tokio::time::sleep(Duration::from_secs(u64::from(p.config.interval_seconds))).await;
            Ok(())
        })
        .await?;
    }
}
fn same_grant(a: &BrokerV1, b: &BrokerV1) -> bool {
    let mut a = a.clone();
    a.heartbeat_revision = b.heartbeat_revision;
    a.last_heartbeat = b.last_heartbeat.clone();
    a == *b
}
pub(super) fn validate_handshake(
    p: &PreparedPresence,
    identity: &BrokerIdentityViewV1,
    view: &BrokerViewV1,
) -> Result<()> {
    if identity.version != 1
        || !matches!(identity.schema_version, 6 | 7 | 8)
        || identity.tenant_id != p.scope.expected_tenant_id
        || identity.control_namespace_uuid != p.scope.control_namespace_uuid.to_string()
        || identity.broker_id != p.config.broker_id
        || identity.credential_id != p.config.credential_id
        || identity.incarnation_id != p.config.incarnation_id
        || identity.binding_revision == 0
        || identity.grant_revision == 0
    {
        return Err(PresenceError::Forbidden);
    }
    revision(identity.binding_revision).map_err(|_| PresenceError::Protocol)?;
    revision(identity.grant_revision).map_err(|_| PresenceError::Protocol)?;
    let broker = &view.broker;
    if broker.version != 1
        || broker.broker_id != identity.broker_id
        || broker.credential_id != identity.credential_id
        || broker.incarnation_id != identity.incarnation_id
        || broker.grant_revision != identity.grant_revision
        || broker.authorities
            != vec![BrokerAuthorityV1 {
                authority_id: p.config.authority_id.clone(),
                runtime_kind: p.scope.runtime_kind,
            }]
        || broker.lease_seconds < p.config.interval_seconds * 2
        || broker.lease_seconds > 300
        || broker.revoked
        || broker.revoked_at.is_some()
        || view.runtime_inventory_complete
    {
        return Err(PresenceError::Forbidden);
    }
    revision(broker.heartbeat_revision).map_err(|_| PresenceError::Protocol)?;
    for timestamp in [&view.evaluated_at, &broker.granted_at, &broker.not_after] {
        parse_time(timestamp).map_err(|_| PresenceError::Protocol)?;
    }
    if let Some(hb) = &broker.last_heartbeat {
        uuid(&hb.incarnation_id).map_err(|_| PresenceError::Protocol)?;
        if hb.sequence == 0 || broker.heartbeat_revision == 0 {
            return Err(PresenceError::Protocol);
        }
        revision(hb.sequence).map_err(|_| PresenceError::Protocol)?;
        let received = parse_time(&hb.received_at).map_err(|_| PresenceError::Protocol)?;
        let expires = parse_time(&hb.lease_expires_at).map_err(|_| PresenceError::Protocol)?;
        if expires < received {
            return Err(PresenceError::Protocol);
        }
    } else if broker.heartbeat_revision != 0 {
        return Err(PresenceError::Protocol);
    }
    let expected = broker
        .view_at(&view.evaluated_at)
        .map_err(|_| PresenceError::Protocol)?;
    if *view != expected
        || !matches!(
            view.state,
            BrokerStateV1::Unseen | BrokerStateV1::Current | BrokerStateV1::Stale
        )
    {
        return Err(if view.state == BrokerStateV1::ClockUncertain {
            PresenceError::Clock
        } else {
            PresenceError::Forbidden
        });
    }
    Ok(())
}
