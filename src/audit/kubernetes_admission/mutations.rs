use super::*;
use crate::broker_registry::BrokerIdentityV1;
use crate::broker_registry::format_time;
use authorization::KubernetesCreateAuthorization;
use authorization::{broker_scope, permission};
use model::{Change, IdentityEvidence, Record, conflict};

/// Created only after the first claim has fully committed. Neither disk nor a
/// deserialized ClaimResponse can construct this proof or recreate freshness.
pub(crate) struct FreshKubernetesCreateClaimCommit {
    tenant_id: String,
    claim: KubernetesCreateClaimV1,
    receipt: KubernetesCreateReceiptV1,
    snapshot: KubernetesCreateSnapshotV1,
}
impl FreshKubernetesCreateClaimCommit {
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn claim(&self) -> &KubernetesCreateClaimV1 {
        &self.claim
    }
    pub(crate) fn receipt(&self) -> &KubernetesCreateReceiptV1 {
        &self.receipt
    }
    pub(crate) fn snapshot(&self) -> &KubernetesCreateSnapshotV1 {
        &self.snapshot
    }
}
/// Exact central ACK, possibly historical; it authorizes no runtime effect.
pub(crate) struct ConfirmedKubernetesCreateResult {
    tenant_id: String,
    result: KubernetesCreateResultV1,
    receipt: KubernetesCreateReceiptV1,
    snapshot: KubernetesCreateSnapshotV1,
}
impl ConfirmedKubernetesCreateResult {
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn result(&self) -> &KubernetesCreateResultV1 {
        &self.result
    }
    pub(crate) fn receipt(&self) -> &KubernetesCreateReceiptV1 {
        &self.receipt
    }
    pub(crate) fn snapshot(&self) -> &KubernetesCreateSnapshotV1 {
        &self.snapshot
    }
}
type MutationCommit = (KubernetesCreateReceiptV1, KubernetesCreateSnapshotV1, bool);
type ClaimCommit = (
    KubernetesCreateClaimResponseV1,
    Option<FreshKubernetesCreateClaimCommit>,
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HeldChangeAccess {
    Mutate,
    ObserveClaim,
}

fn publication_is_eligible(
    replayed: bool,
    window: Option<&KubernetesCreateEmissionWindowV1>,
    at: chrono::DateTime<chrono::Utc>,
) -> bool {
    if replayed {
        return false;
    }
    let Some((issued, end)) = window.and_then(|window| {
        Some((
            crate::broker_registry::parse_time(&window.issued_at).ok()?,
            crate::broker_registry::parse_time(&window.not_after).ok()?,
        ))
    }) else {
        return false;
    };
    issued <= at && at < end && end - issued <= chrono::Duration::seconds(8)
}

impl Store {
    /// Trusted API glue only: resolve an owned retained intent before issuing
    /// a current Admission capability. This grants no claim or runtime effect.
    pub(crate) fn kubernetes_create_intent_for_admission(
        &self,
        identity: &BrokerIdentityV1,
        create_id: &str,
    ) -> anyhow::Result<KubernetesCreateIntentV1> {
        crate::control::uuid(create_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if ![9, 10].contains(&version) {
            return Err(KubernetesCreateFailureV1::MigrationRequired.into());
        }
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let snapshot =
            replay::load_snapshot(&tx, create_id)?.ok_or(KubernetesCreateFailureV1::NotFound)?;
        broker_scope(&tx, identity, &snapshot.intent)?;
        self.commit_authorized(tx, &clock)?;
        Ok(snapshot.intent)
    }

    pub(crate) fn reserve_kubernetes_create(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        intent: &KubernetesCreateIntentV1,
    ) -> anyhow::Result<MutationCommit> {
        self.persist_held_create(
            identity,
            authorization,
            Change::Reserve {
                intent: intent.clone(),
            },
        )
    }
    pub(crate) fn claim_kubernetes_create(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        claim: &KubernetesCreateClaimV1,
    ) -> anyhow::Result<ClaimCommit> {
        let (receipt, snapshot, replayed) = self.persist_held_create(
            identity,
            authorization,
            Change::Claim {
                claim: claim.clone(),
            },
        )?;
        // Publication is after the entire checked commit, including late checks.
        let eligible = publication_is_eligible(
            replayed,
            receipt.emission_window.as_ref(),
            chrono::Utc::now(),
        );
        let fresh = eligible.then(|| FreshKubernetesCreateClaimCommit {
            tenant_id: self.tenant_id.clone(),
            claim: claim.clone(),
            receipt: receipt.clone(),
            snapshot: snapshot.clone(),
        });
        Ok((
            KubernetesCreateClaimResponseV1 {
                version: 1,
                receipt,
                snapshot,
                disposition: if eligible {
                    KubernetesCreateDispositionV1::EmitOnce
                } else {
                    KubernetesCreateDispositionV1::ObserveOnly
                },
            },
            fresh,
        ))
    }
    pub(crate) fn record_kubernetes_create_result(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        result: &KubernetesCreateResultV1,
    ) -> anyhow::Result<ConfirmedKubernetesCreateResult> {
        let (receipt, snapshot, _) = self.persist_held_create(
            identity,
            authorization,
            Change::Result {
                result: result.clone(),
            },
        )?;
        Ok(ConfirmedKubernetesCreateResult {
            tenant_id: self.tenant_id.clone(),
            result: result.clone(),
            receipt,
            snapshot,
        })
    }
    pub(crate) fn cancel_kubernetes_create(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        cancel: &KubernetesCreateCancelV1,
    ) -> anyhow::Result<MutationCommit> {
        self.persist_held_create(
            identity,
            authorization,
            Change::Cancel {
                cancel: cancel.clone(),
            },
        )
    }

    /// Authenticated historical lookup only. A missing operation is never
    /// created, supervision is never activated and no fresh proof can result.
    pub(crate) fn observe_kubernetes_create_claim_operation(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        create_id: &str,
        operation_id: &str,
    ) -> anyhow::Result<KubernetesCreateClaimResponseV1> {
        crate::control::uuid(create_id)?;
        crate::control::uuid(operation_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_capability_schema(&tx, &authorization.evidence)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(
            identity.tenant_id() == self.tenant_id
                && authorization.evidence.tenant_id() == &self.tenant_id,
        )?;
        let snapshot =
            replay::load_snapshot(&tx, create_id)?.ok_or(KubernetesCreateFailureV1::NotFound)?;
        authorization
            .evidence
            .validate(identity, &snapshot.intent, clock.at())?;
        // Scope is checked before the operation-id lookup, independently of
        // whether that historical claim happens to exist.
        authorization
            .evidence
            .validate_observe_access(HeldChangeAccess::ObserveClaim)?;
        broker_scope(&tx, identity, &snapshot.intent)?;
        replay::validate_current_basis(
            &tx,
            &authorization.evidence,
            &snapshot.intent,
            &self.tenant_id,
        )?;
        let stored = tx
            .query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                [operation_id],
                row_event,
            )
            .optional()?
            .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let record = replay::record(&stored)?;
        let Change::Claim { claim } = &record.change else {
            return Err(KubernetesCreateFailureV1::Conflict.into());
        };
        conflict(
            claim.operation_id == operation_id
                && claim.create_id == create_id
                && record.intent == snapshot.intent
                && claim.intent_hash == snapshot.intent_hash,
        )?;
        authorization
            .evidence
            .validate_access(&record.change, HeldChangeAccess::ObserveClaim)?;
        replay::validate_historical_identity(&tx, &stored, &record, &self.tenant_id)?;
        let response = KubernetesCreateClaimResponseV1 {
            version: 1,
            receipt: model::receipt(&stored, &record),
            snapshot: record.snapshot,
            disposition: KubernetesCreateDispositionV1::ObserveOnly,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(response)
    }

    /// Authenticated historical lookup only. A missing operation is never
    /// created, supervision is never activated and no fresh proof can result.
    pub(crate) fn observe_kubernetes_create_claim(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        claim: &KubernetesCreateClaimV1,
    ) -> anyhow::Result<KubernetesCreateClaimResponseV1> {
        let (receipt, snapshot, replayed) = self.held_create_change(
            identity,
            authorization,
            Change::Claim {
                claim: claim.clone(),
            },
            HeldChangeAccess::ObserveClaim,
        )?;
        conflict(replayed)?;
        Ok(KubernetesCreateClaimResponseV1 {
            version: 1,
            receipt,
            snapshot,
            disposition: KubernetesCreateDispositionV1::ObserveOnly,
        })
    }

    fn persist_held_create(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        change: Change,
    ) -> anyhow::Result<MutationCommit> {
        self.held_create_change(identity, authorization, change, HeldChangeAccess::Mutate)
    }
    fn held_create_change(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesCreateAuthorization,
        change: Change,
        access: HeldChangeAccess,
    ) -> anyhow::Result<MutationCommit> {
        change.validate()?;
        permission(
            access != HeldChangeAccess::ObserveClaim || matches!(change, Change::Claim { .. }),
        )?;
        permission(
            identity.tenant_id() == self.tenant_id
                && authorization.evidence.tenant_id() == &self.tenant_id,
        )?;
        let input = serde_json::to_value(&change)?;
        permission(self.redactor.value(&input) == input)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_capability_schema(&tx, &authorization.evidence)?;
        authorization.evidence.validate_access(&change, access)?;
        let clock = workload::capture_control_time(&tx)?;
        let now = clock.at();
        workload::revalidate_broker_identity(&tx, identity, now)?;
        if access == HeldChangeAccess::Mutate && change.requires_active() && !tx.lease.active {
            return Err(KubernetesCreateFailureV1::SupervisionInactive.into());
        }
        let previous = replay::load_snapshot(&tx, change.create_id())?;
        let intent = match &change {
            Change::Reserve { intent } => intent.clone(),
            _ => previous
                .as_ref()
                .ok_or(KubernetesCreateFailureV1::NotFound)?
                .intent
                .clone(),
        };
        authorization.evidence.validate(identity, &intent, now)?;
        let broker = broker_scope(&tx, identity, &intent)?;
        replay::validate_current_basis(&tx, &authorization.evidence, &intent, &self.tenant_id)?;

        // Global event identity, not a separate permissive operations ledger.
        let old = tx
            .query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                [change.operation_id()],
                row_event,
            )
            .optional()?;
        if let Some(stored) = old {
            conflict(stored.event.kind == change.kind())?;
            let record = replay::record(&stored)?;
            conflict(
                record.input_hash == change.input_hash()?
                    && record.change == change
                    && record.intent == intent,
            )?;
            replay::validate_historical_identity(&tx, &stored, &record, &self.tenant_id)?;
            let receipt = model::receipt(&stored, &record);
            self.commit_authorized(tx, &clock)?;
            return Ok((receipt, record.snapshot, true));
        }
        if access == HeldChangeAccess::ObserveClaim {
            return Err(KubernetesCreateFailureV1::NotFound.into());
        }
        if matches!(change, Change::Reserve { .. }) {
            let duplicate: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workers WHERE session_id=?1) OR EXISTS(SELECT 1 FROM kubernetes_creates WHERE session_id=?1)",
                [&intent.session_id], |row| row.get(0))?;
            conflict(!duplicate)?;
        }
        if change.requires_active() {
            let usage = super::super::control::quota_usage(&tx, authorization.evidence.quota())?;
            replay::ensure_quota(&usage, &intent, matches!(change, Change::Reserve { .. }))?;
        }
        let window = if matches!(change, Change::Claim { .. }) {
            Some(model::emission_window(
                now,
                authorization.evidence.expires_at(),
                &broker.not_after,
            )?)
        } else {
            None
        };
        let snapshot = model::transition(&intent, previous.as_ref(), &change, window)?;
        let record = Record {
            version: *authorization.evidence.version(),
            input_hash: change.input_hash()?,
            received_at: format_time(now),
            identity: IdentityEvidence::capture(identity),
            authorization: authorization.evidence.clone(),
            intent,
            previous_revision: previous.as_ref().map_or(0, |snapshot| snapshot.revision),
            change,
            snapshot,
        };
        let mut event = Event::new(record.change.kind());
        event.id = record.change.operation_id().into();
        event.timestamp = record.received_at.clone();
        if matches!(record.change, Change::Result { .. }) {
            event.provenance = "declared".into();
            event.coverage = "partial".into();
        }
        event.data = serde_json::to_value(&record)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        permission(serde_json::to_value(&event)? == original)?;
        let stored = self.append_authorized_workload_in_transaction(
            &tx,
            event,
            payload,
            capacity::WorkloadAuditPrincipalV1::Broker {
                broker_id: identity.broker_id().into(),
            },
            &clock,
        )?;
        replay::write_snapshot(
            &tx,
            &record.snapshot,
            stored.seq,
            previous.as_ref().map(|snapshot| snapshot.revision),
        )?;
        let receipt = model::receipt(&stored, &record);
        self.commit_authorized(tx, &clock)?;
        Ok((receipt, record.snapshot, false))
    }
}

#[cfg(test)]
#[path = "mutation_tests.rs"]
mod tests;
