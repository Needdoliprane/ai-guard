//! Retained-body accounting for relay admission.
//!
//! Occupancy is derived from rows whose body is non-NULL inside the checked
//! write transaction: there is no counter table to drift and no memory cache
//! that decides. Acknowledged and expired bodies count for as long as they are
//! present in those rows. This does not bound SQLite/WAL file size or inodes,
//! nor promise secure erasure when a body is set to NULL.
use super::*;
use crate::audit::workload::{AuthorizedControlTimeV1, capture_control_time};

/// One bounded pass over retained bodies. `metadata_json` precedes `body` in
/// the row, so reading it does not load the retained content.
pub(super) const RETAINED_SNAPSHOT_SQL: &str = "SELECT metadata_json FROM relay_messages INDEXED BY relay_messages_retained WHERE body IS NOT NULL LIMIT 1025";

/// Occupancy of one scope and the earliest expiry among its retained bodies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Usage {
    pub(super) retained: RelayCapacityV1,
    pub(super) next_expiry: Option<DateTime<Utc>>,
    /// At least one body is neither expired nor acknowledged: the relay still
    /// accepts its acknowledgement. Whether its recipient may still send one
    /// (grant not revoked, endpoint still authorized) is not evaluated here.
    pub(super) awaits_ack: bool,
}

#[derive(Debug)]
pub(super) struct RetainedBody {
    pub(super) metadata: RelayMessageMetadataV1,
    pub(super) expires: DateTime<Utc>,
}

/// Every physically retained body of the tenant, at most the compiled bound.
#[derive(Debug, Default)]
pub(super) struct Retained {
    pub(super) bodies: Vec<RetainedBody>,
}
impl Retained {
    pub(super) fn load(db: &Connection) -> anyhow::Result<Self> {
        let mut statement = db.prepare(RETAINED_SNAPSHOT_SQL)?;
        let mut rows = statement.query([])?;
        let mut bodies = Vec::new();
        let mut bytes = 0_u64;
        while let Some(row) = rows.next()? {
            let metadata: RelayMessageMetadataV1 = serde_json::from_str(&row.get::<_, String>(0)?)?;
            anyhow::ensure!(
                !metadata.body_pruned,
                "retained relay body is recorded as pruned"
            );
            bytes = bytes
                .checked_add(metadata.body_bytes)
                .context("relay byte overflow")?;
            bodies.push(RetainedBody {
                expires: timestamp(&metadata.expires_at)?,
                metadata,
            });
        }
        anyhow::ensure!(
            bodies.len() as u64 <= MAX_RETAINED_MESSAGES,
            "retained relay bodies exceed the physical bound"
        );
        // The typed pass and the indexed projection must describe the same rows.
        let (count, indexed): (i64, i64) =
            db.query_row(RETAINED_USAGE_SQL, [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        anyhow::ensure!(
            u64::try_from(count)? == bodies.len() as u64 && u64::try_from(indexed)? == bytes,
            "retained relay metadata differs from its size projection"
        );
        Ok(Self { bodies })
    }

    /// Usage of the bodies selected by `scope`, with expiry evaluated at `now`.
    pub(super) fn usage(
        &self,
        now: DateTime<Utc>,
        scope: impl Fn(&RelayMessageMetadataV1) -> bool,
    ) -> Usage {
        let mut usage = Usage::default();
        for body in self.bodies.iter().filter(|body| scope(&body.metadata)) {
            usage.retained.messages += 1;
            // Bounded by the checked total of at most 1024 bodies of 64 KiB.
            usage.retained.bytes += body.metadata.body_bytes;
            usage.next_expiry = Some(
                usage
                    .next_expiry
                    .map_or(body.expires, |known| known.min(body.expires)),
            );
            // Same boundary as `relay_ack`: a body expiring exactly now is expired.
            usage.awaits_ack |= !body.metadata.acknowledged && body.expires > now;
        }
        usage
    }
}

impl Retained {
    /// Expired bodies in release order: the sender's own first, then the
    /// oldest of the tenant, so bodies of departed senders drain too.
    pub(super) fn release_order(
        &self,
        source: &RelayEndpointV1,
        now: DateTime<Utc>,
        batch: u32,
    ) -> Vec<String> {
        let mut expired: Vec<_> = self
            .bodies
            .iter()
            .filter(|body| body.expires <= now)
            .map(|body| {
                (
                    body.metadata.source.worker_id != source.worker_id,
                    body.expires,
                    body.metadata.message_id.as_str(),
                )
            })
            .collect();
        expired.sort_unstable();
        expired
            .into_iter()
            .take(batch as usize)
            .map(|(_, _, message_id)| message_id.to_owned())
            .collect()
    }
}

/// Occupancy of one diagnostic scope, with its expired subset.
#[derive(Default)]
struct Occupancy {
    retained: RelayCapacityV1,
    expired: RelayCapacityV1,
    next_expiry: Option<DateTime<Utc>>,
}
impl Occupancy {
    fn add(&mut self, body: &RetainedBody, now: DateTime<Utc>) {
        count(&mut self.retained, body);
        if body.expires <= now {
            count(&mut self.expired, body);
        }
        self.next_expiry = Some(
            self.next_expiry
                .map_or(body.expires, |known| known.min(body.expires)),
        );
    }
    fn next_expiry_at(&self) -> Option<String> {
        self.next_expiry.map(canonical_time)
    }
}
fn count(capacity: &mut RelayCapacityV1, body: &RetainedBody) {
    capacity.messages += 1;
    // Bounded by the checked total of at most 1024 bodies of 64 KiB.
    capacity.bytes += body.metadata.body_bytes;
}

impl Retained {
    /// Diagnostic projection at `now`. Lists are truncated to `limit` rows and
    /// their totals keep the truncation visible. No body is read or returned.
    pub(super) fn status(
        &self,
        policy: &RelayRetentionPolicyV1,
        now: DateTime<Utc>,
        limit: u32,
    ) -> anyhow::Result<RelayRetentionStatusV1> {
        let limit = limit as usize;
        let mut tenant = Occupancy::default();
        let mut acknowledged = RelayCapacityV1::default();
        let mut senders: BTreeMap<&str, Occupancy> = BTreeMap::new();
        let mut grants: BTreeMap<&str, (&RelayMessageMetadataV1, Occupancy)> = BTreeMap::new();
        let mut prunable = Vec::new();
        for body in &self.bodies {
            let message = &body.metadata;
            tenant.add(body, now);
            if message.acknowledged {
                count(&mut acknowledged, body);
            }
            senders
                .entry(&message.source.worker_id)
                .or_default()
                .add(body, now);
            grants
                .entry(&message.grant_id)
                .or_insert_with(|| (message, Occupancy::default()))
                .1
                .add(body, now);
            if body.expires <= now {
                prunable.push(body);
            }
        }
        let mut senders: Vec<_> = senders.into_iter().collect();
        senders.sort_by(|a, b| (b.1.retained.bytes, a.0).cmp(&(a.1.retained.bytes, b.0)));
        let mut grants: Vec<_> = grants.into_iter().collect();
        grants.sort_by(|a, b| (b.1.1.retained.bytes, a.0).cmp(&(a.1.1.retained.bytes, b.0)));
        prunable.sort_by(|a, b| {
            (a.expires, &a.metadata.message_id).cmp(&(b.expires, &b.metadata.message_id))
        });
        Ok(RelayRetentionStatusV1 {
            version: 1,
            observed_at: canonical_time(now),
            policy: policy.clone(),
            policy_fingerprint: policy.fingerprint()?,
            tenant: RelayTenantRetentionV1 {
                limit: policy.tenant,
                retained: tenant.retained,
                expired: tenant.expired,
                acknowledged,
                next_expiry_at: tenant.next_expiry_at(),
            },
            senders_total: senders.len() as u64,
            senders: senders
                .into_iter()
                .take(limit)
                .map(|(worker_id, usage)| RelaySenderRetentionV1 {
                    worker_id: worker_id.to_owned(),
                    limit: policy.sender,
                    retained: usage.retained,
                    expired: usage.expired,
                    next_expiry_at: usage.next_expiry_at(),
                })
                .collect(),
            grants_total: grants.len() as u64,
            grants: grants
                .into_iter()
                .take(limit)
                .map(|(grant_id, (message, usage))| RelayGrantRetentionV1 {
                    grant_id: grant_id.to_owned(),
                    source_worker_id: message.source.worker_id.clone(),
                    destination_worker_id: message.destination.worker_id.clone(),
                    channel: message.channel.clone(),
                    limit: policy.grant,
                    retained: usage.retained,
                    expired: usage.expired,
                    next_expiry_at: usage.next_expiry_at(),
                })
                .collect(),
            prunable_total: prunable.len() as u64,
            prunable: prunable
                .into_iter()
                .take(limit)
                .map(|body| RelayPrunableBodyV1 {
                    message_id: body.metadata.message_id.clone(),
                    grant_id: body.metadata.grant_id.clone(),
                    source_worker_id: body.metadata.source.worker_id.clone(),
                    destination_worker_id: body.metadata.destination.worker_id.clone(),
                    body_bytes: body.metadata.body_bytes,
                    expires_at: body.metadata.expires_at.clone(),
                    acknowledged: body.metadata.acknowledged,
                })
                .collect(),
        })
    }
}

/// Result of the bounded release pass that precedes admission.
pub(super) struct Cleanup {
    /// Latest authorized instant. It dates the next fact and the commit.
    pub(super) clock: AuthorizedControlTimeV1,
    pub(super) released: u32,
    /// Audit head once the last release fact is written.
    head: (i64, String),
}

/// Capacity advice describes what the policy permits for this own scope.
/// Each future write still requires its audit fact. With only acknowledgement
/// release, a live, unacknowledged body is needed for that release to help.
fn own_retry_action(policy: &RelayRetentionPolicyV1, refused: &Usage) -> RelayRetryActionV1 {
    let by_sends = policy.automatic_prune_batch > 0;
    let by_acknowledgement = policy.release_on_ack && refused.awaits_ack;
    if by_sends || by_acknowledgement {
        RelayRetryActionV1::RetryAfterRelease
    } else {
        RelayRetryActionV1::ContactOperator
    }
}

impl Cleanup {
    /// Completed, audited releases stay in place when the send itself receives
    /// a typed refusal and wrote nothing after them. Any other error, or any
    /// fact beyond the releases, rolls the whole transaction back.
    pub(super) fn survives(&self, tx: &Transaction<'_>, error: &anyhow::Error) -> bool {
        let typed_refusal = error.downcast_ref::<RelayCapacityRefusalV1>().is_some()
            || error
                .downcast_ref::<RelayTenantCapacityRefusalV1>()
                .is_some()
            || error.downcast_ref::<RelayFailure>().is_some()
            || error.downcast_ref::<AuditCapacityFailureV1>().is_some();
        self.released > 0 && typed_refusal && audit_head(tx).is_ok_and(|head| head == self.head)
    }
}

fn exceeded(
    limit: RelayCapacityV1,
    retained: RelayCapacityV1,
    requested_bytes: u64,
) -> Option<RelayCapacityDimensionV1> {
    if retained.messages >= limit.messages {
        Some(RelayCapacityDimensionV1::Messages)
    } else if retained
        .bytes
        .checked_add(requested_bytes)
        .is_none_or(|next| next > limit.bytes)
    {
        Some(RelayCapacityDimensionV1::Bytes)
    } else {
        None
    }
}

/// Admit one new body of `requested_bytes` from `source` over `grant_id`.
///
/// The sender scope is checked first: switching grants cannot help a sender
/// whose own share is full. The tenant ceiling comes last; its refusal carries
/// an action and no figure, because any would describe neighbors.
///
/// `now` is the instant of the send. Advice follows the policy and, for a
/// sender or grant refusal, that scope's own usage at `now`.
pub(super) fn admit(
    policy: &RelayRetentionPolicyV1,
    retained: &Retained,
    source: &RelayEndpointV1,
    grant_id: &str,
    requested_bytes: u64,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let own = [
        (
            RelayCapacityScopeV1::Sender,
            policy.sender,
            retained.usage(now, |message| message.source.worker_id == source.worker_id),
        ),
        (
            RelayCapacityScopeV1::Grant,
            policy.grant,
            retained.usage(now, |message| message.grant_id == grant_id),
        ),
    ];
    for (scope, limit, usage) in own {
        if let Some(dimension) = exceeded(limit, usage.retained, requested_bytes) {
            return Err(RelayCapacityRefusalV1 {
                scope,
                dimension,
                limit,
                retained: usage.retained,
                requested_bytes,
                next_release_at: usage.next_expiry.map(canonical_time),
                retry_action: own_retry_action(policy, &usage),
            }
            .into());
        }
    }
    let tenant = retained.usage(now, |_| true);
    if exceeded(policy.tenant, tenant.retained, requested_bytes).is_some() {
        return Err(RelayTenantCapacityRefusalV1 {
            // The policy determines tenant advice independently of other
            // senders' retained bodies and the outcome of a release pass.
            retry_action: if policy.automatic_prune_batch > 0 {
                RelayRetryActionV1::RetryAfterRelease
            } else {
                RelayRetryActionV1::ContactOperator
            },
        }
        .into());
    }
    Ok(())
}

impl Store {
    /// Select the immutable retention policy before creating any sibling or Arc.
    pub fn with_relay_retention_policy(
        mut self,
        policy: RelayRetentionPolicyV1,
    ) -> anyhow::Result<Self> {
        policy.validate()?;
        anyhow::ensure!(
            Arc::strong_count(&self.supervision_lease) == 1,
            "relay retention policy must be selected before sharing the Store"
        );
        self.relay_retention_policy = policy;
        Ok(self)
    }
    pub fn relay_retention_policy(&self) -> &RelayRetentionPolicyV1 {
        &self.relay_retention_policy
    }
    pub fn relay_retention_policy_fingerprint(&self) -> anyhow::Result<String> {
        self.relay_retention_policy.fingerprint()
    }
    /// Read a closed policy from a private, bounded operator file. The caller
    /// validates it before any Store is opened; nothing is reloaded later.
    pub fn load_relay_retention_policy(path: &Path) -> anyhow::Result<RelayRetentionPolicyV1> {
        let path = crate::control_fs::absolute_path(path)?;
        let held = crate::control_fs::PrivateFile::open(&path, 16_384)?;
        let policy =
            serde_json::from_slice(held.bytes()).context("invalid relay retention policy file")?;
        held.revalidate()?;
        Ok(policy)
    }

    /// Read-only operator diagnostic: scoped usage, expired bodies still
    /// retained and the applied policy. It writes no audit event, returns no
    /// body and works while supervision is inactive, as after a restart.
    pub fn relay_retention_status(&self, limit: u32) -> anyhow::Result<RelayRetentionStatusV1> {
        self.relay_retention_status_with_access(None, limit)
    }

    pub(crate) fn relay_retention_status_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        limit: u32,
    ) -> anyhow::Result<RelayRetentionStatusV1> {
        self.relay_retention_status_with_access(Some(access), limit)
    }

    fn relay_retention_status_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        limit: u32,
    ) -> anyhow::Result<RelayRetentionStatusV1> {
        valid((1..=MAX_RETENTION_STATUS_ITEMS).contains(&limit))?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_relay(&tx)?;
        // Expiry is classified at an instant checked against durable evidence.
        let clock = capture_control_time(&tx)?;
        let status =
            Retained::load(&tx)?.status(&self.relay_retention_policy, clock.at(), limit)?;
        let value = serde_json::to_value(&status)?;
        anyhow::ensure!(
            self.redactor.value_unclipped(&value) == value,
            "relay retention status requires redaction"
        );
        self.commit_authorized(tx, &clock)?;
        Ok(status)
    }

    /// Release at most `automatic_prune_batch` expired bodies inside an
    /// authorized send, each by one metadata-only `prune` fact that satisfies
    /// the historical rule (no actor, expiry at or before the fact).
    ///
    /// `source` is the verified identity of the sender whose send triggers the
    /// pass; it is recorded as provenance beside `trigger`, never as an actor.
    ///
    /// One fact, one clock: a clock that already dates a fact is never reused.
    /// The returned clock is fresh and dates whatever follows.
    pub(super) fn relay_release_expired(
        &self,
        tx: &Transaction<'_>,
        retained: &mut Retained,
        source: &RelayEndpointV1,
        trigger: &str,
        mut clock: AuthorizedControlTimeV1,
    ) -> anyhow::Result<Cleanup> {
        let batch = self.relay_retention_policy.automatic_prune_batch;
        let mut released = 0;
        let mut dated = false;
        for message_id in retained.release_order(source, clock.at(), batch) {
            if dated {
                clock = capture_control_time(tx)?;
                dated = false;
            }
            let Some(index) = retained
                .bodies
                .iter()
                .position(|body| body.metadata.message_id == message_id)
            else {
                continue;
            };
            if retained.bodies[index].expires > clock.at() {
                // Not expired at this fact's own instant.
                break;
            }
            let mut value = retained.bodies[index].metadata.clone();
            value.body_pruned = true;
            let operation_id = uuid::Uuid::new_v4().to_string();
            let record = Record {
                version: 1,
                input_hash: input_hash(
                    "prune",
                    &RelayPruneV1 {
                        version: 1,
                        operation_id: operation_id.clone(),
                        message_id: value.message_id.clone(),
                    },
                )?,
                operation_id,
                action: "prune".into(),
                actor: None,
                resource: Resource::Message(value),
            };
            match self.persist_relay(
                tx,
                &record,
                None,
                &clock,
                Origin::AutomaticPrune {
                    trigger,
                    worker: &source.worker_id,
                },
            ) {
                Ok(_) => {
                    dated = true;
                    released += 1;
                    retained.bodies.swap_remove(index);
                }
                // Refused before any write: stop releasing, mutate nothing more.
                Err(error)
                    if error.downcast_ref::<AuditCapacityFailureV1>()
                        == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if dated {
            clock = capture_control_time(tx)?;
        }
        Ok(Cleanup {
            clock,
            released,
            head: audit_head(tx)?,
        })
    }
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "retention_fault_tests.rs"]
mod fault_tests;
