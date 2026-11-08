use super::*;
use authorization::{Action, Basis, Evidence, RecoveryAction};
use model::{Actor, Change, Creator, Record};

fn exact_operation(
    db: &Connection,
    evidence: &Evidence,
    intent: &KubernetesSessionReservationIntentV2,
    change: &Change,
) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let old = replay::operation(db, change.operation_id(intent))?;
    if let Some((stored, record)) = &old {
        conflict(
            record.change == *change
                && record.input_hash == change.input_hash(intent, &evidence.actor.tenant_id)?
                && record.intent.same(intent)?,
        )?;
        replay::validate_historical(db, stored, record, &evidence.actor.tenant_id)?;
    }
    Ok(old)
}

fn prepared(
    store: &Store,
    db: &Connection,
    evidence: &Evidence,
    intent: &KubernetesSessionReservationIntentV2,
    change: &Change,
    at: DateTime<Utc>,
) -> anyhow::Result<(Record, Event, String)> {
    evidence.validate(intent, &evidence.actor, at)?;
    evidence.allows(change, intent)?;
    let previous = replay::load_snapshot(db, intent.reservation_id(), &evidence.actor.tenant_id)?;
    let creator = match change {
        Change::Reserve => Creator::ThisReserve,
        Change::CloseBeforeEffect { .. } => {
            let (stored, original) =
                replay::reservation(db, intent.reservation_id(), &evidence.actor.tenant_id)?;
            conflict(original.intent.same(intent)?)?;
            Creator::Reservation {
                reservation_seq: stored.seq,
                reservation_hash: stored.hash,
                actor: original.actor.identity(),
            }
        }
    };
    let state = model::transition(
        intent,
        previous.as_ref().map(|(snapshot, _)| &snapshot.state),
        change,
        &evidence.actor.tenant_id,
    )?;
    let record = Record {
        version: 2,
        input_hash: change.input_hash(intent, &evidence.actor.tenant_id)?,
        received_at: format_time(at),
        actor: Actor::broker(&evidence.actor),
        creator,
        authorization: evidence.clone(),
        intent: intent.clone(),
        previous_revision: previous
            .as_ref()
            .map_or(0, |(snapshot, _)| snapshot.state.revision),
        change: change.clone(),
        post_state: state,
    };
    let event = model::event(&record)?;
    let original = serde_json::to_value(&event)?;
    // Bound the entire retained event, including policy, creator and authority.
    require(serde_json::to_vec(&event)?.len() <= 128 * 1024)?;
    let (event, payload) = store.prepare_event(event)?;
    permission(serde_json::to_value(&event)? == original)?;
    require(payload.len() <= 128 * 1024)?;
    Ok((record, event, payload))
}

/// Issuers and mutations share the same complete-record bound. Exact replay
/// neither reconstructs a new record nor spends quota or workload capacity.
pub(super) fn preflight(
    store: &Store,
    db: &Connection,
    evidence: &Evidence,
    intent: &KubernetesSessionReservationIntentV2,
    change: &Change,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    evidence.validate(intent, &evidence.actor, at)?;
    evidence.allows(change, intent)?;
    if exact_operation(db, evidence, intent, change)?.is_none() {
        prepared(store, db, evidence, intent, change, at)?;
    }
    Ok(())
}

fn current_evidence(
    db: &Connection,
    identity: &BrokerIdentityV1,
    evidence: &Evidence,
    intent: &KubernetesSessionReservationIntentV2,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let successor = matches!(
        evidence.basis,
        Basis::RecoverBeforeEffect { .. } | Basis::RecoveryReceiptLookup { .. }
    );
    let broker = authorization::current(db, identity, intent, tenant, successor, at)?;
    permission(evidence.actor == model::IdentityEvidence::capture(identity, &broker))?;
    evidence.validate(intent, &evidence.actor, at)?;
    replay::validate_origin(db, evidence, None)
}

impl Store {
    pub(crate) fn reserve_kubernetes_session(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionReservationAuthorizationV2,
        intent: &KubernetesSessionReservationIntentV2,
        policy: &KubernetesSessionReservationPolicyV2,
        quota: &crate::control::QuotaPolicyV1,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        let Basis::Reserve { policy_hash, .. } = &authorization.evidence.basis else {
            return Err(Failure::PermissionDenied.into());
        };
        permission(
            *policy_hash == policy.policy_hash()? && &authorization.evidence.quota == quota,
        )?;
        self.persist_kubernetes_session(identity, &authorization.evidence, intent, Change::Reserve)
    }
    pub(crate) fn close_kubernetes_session_before_effect(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionReservationAuthorizationV2,
        close: &KubernetesSessionReservationCloseV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        permission(matches!(
            authorization.evidence.basis,
            Basis::Reconcile {
                action: Action::CloseBeforeEffect,
                ..
            }
        ))?;
        self.close_session_with_evidence(identity, &authorization.evidence, close)
    }
    pub(crate) fn recover_kubernetes_session_before_effect(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionRecoverBeforeEffectV2,
        close: &KubernetesSessionReservationCloseV2,
        policy: &KubernetesSessionRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        let Basis::RecoverBeforeEffect {
            action: RecoveryAction::CloseReserved,
            policy_hash,
            ..
        } = &authorization.evidence.basis
        else {
            return Err(Failure::PermissionDenied.into());
        };
        permission(*policy_hash == policy.policy_hash()?)?;
        self.close_session_with_evidence(identity, &authorization.evidence, close)
    }
    fn close_session_with_evidence(
        &self,
        identity: &BrokerIdentityV1,
        evidence: &Evidence,
        close: &KubernetesSessionReservationCloseV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        close.validate()?;
        // Resolve only after fresh identity checks; the capability remains bound
        // to this exact reservation, namespace and historical creator.
        let intent =
            self.kubernetes_session_intent_for_reconciliation(identity, &close.reservation_id)?;
        self.persist_kubernetes_session(
            identity,
            evidence,
            &intent,
            Change::CloseBeforeEffect {
                close: close.clone(),
            },
        )
    }
    pub(crate) fn kubernetes_session_intent_for_reconciliation(
        &self,
        identity: &BrokerIdentityV1,
        id: &str,
    ) -> anyhow::Result<KubernetesSessionReservationIntentV2> {
        uuid(id)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let (_, original) = replay::reservation(&tx, id, &self.tenant_id)?;
        permission(original.actor.identity().broker_id == identity.broker_id())?;
        self.commit_authorized(tx, &clock)?;
        Ok(original.intent)
    }
    fn persist_kubernetes_session(
        &self,
        identity: &BrokerIdentityV1,
        evidence: &Evidence,
        intent: &KubernetesSessionReservationIntentV2,
        change: Change,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        intent.validate()?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        current_evidence(&tx, identity, evidence, intent, &self.tenant_id, clock.at())?;
        evidence.allows(&change, intent)?;
        if let Some((stored, record)) = exact_operation(&tx, evidence, intent, &change)? {
            let response = KubernetesSessionReservationMutationResponseV2 {
                version: 2,
                receipt: model::receipt(&stored, &record)?,
                snapshot: model::snapshot(intent, record.post_state, &self.tenant_id)?,
                replayed: true,
            };
            self.commit_authorized(tx, &clock)?;
            return Ok(response);
        }
        if matches!(change, Change::Reserve) {
            if !tx.lease.active {
                return Err(Failure::SupervisionInactive.into());
            }
            replay::ensure_no_legacy_identity(&tx, intent)?;
            let usage = control::quota_usage(&tx, &evidence.quota)?;
            replay::ensure_quota(&usage, intent, true)?;
        }
        let (record, event, payload) = prepared(self, &tx, evidence, intent, &change, clock.at())?;
        let stored = self.append_authorized_workload_in_transaction(
            &tx,
            event,
            payload,
            capacity::WorkloadAuditPrincipalV1::Broker {
                broker_id: intent.ids()?.broker_id.to_string(),
            },
            &clock,
        )?;
        let snapshot = model::snapshot(intent, record.post_state.clone(), &self.tenant_id)?;
        replay::write_snapshot(
            &tx,
            &snapshot,
            stored.seq,
            (record.previous_revision > 0).then_some(record.previous_revision),
            &self.tenant_id,
        )?;
        let response = KubernetesSessionReservationMutationResponseV2 {
            version: 2,
            receipt: model::receipt(&stored, &record)?,
            snapshot,
            replayed: false,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(response)
    }
    pub(crate) fn observe_kubernetes_session(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionReservationAuthorizationV2,
        id: &str,
    ) -> anyhow::Result<KubernetesSessionReservationCurrentV2> {
        permission(matches!(
            authorization.evidence.basis,
            Basis::Reconcile {
                action: Action::Observe,
                ..
            }
        ))?;
        self.observe_session_with_evidence(identity, &authorization.evidence, id, false)
    }
    pub(crate) fn observe_recoverable_kubernetes_session(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionRecoverBeforeEffectV2,
        id: &str,
        policy: &KubernetesSessionRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationCurrentV2> {
        let Basis::RecoverBeforeEffect {
            action: RecoveryAction::ObserveReserved,
            policy_hash,
            ..
        } = &authorization.evidence.basis
        else {
            return Err(Failure::PermissionDenied.into());
        };
        permission(*policy_hash == policy.policy_hash()?)?;
        self.observe_session_with_evidence(identity, &authorization.evidence, id, true)
    }
    fn observe_session_with_evidence(
        &self,
        identity: &BrokerIdentityV1,
        evidence: &Evidence,
        id: &str,
        reserved: bool,
    ) -> anyhow::Result<KubernetesSessionReservationCurrentV2> {
        uuid(id)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let (snapshot, seq) =
            replay::load_snapshot(&tx, id, &self.tenant_id)?.ok_or(Failure::NotFound)?;
        current_evidence(
            &tx,
            identity,
            evidence,
            &snapshot.intent,
            &self.tenant_id,
            clock.at(),
        )?;
        let target = match &evidence.basis {
            Basis::Reconcile { target_hash, .. }
            | Basis::RecoverBeforeEffect { target_hash, .. } => target_hash,
            _ => return Err(Failure::PermissionDenied.into()),
        };
        permission(
            *target == snapshot.input_hash
                && (!reserved
                    || snapshot.state.state == KubernetesSessionReservationStateV2::Reserved),
        )?;
        let hash: String = tx.query_row("SELECT hash FROM events WHERE seq=?1", [seq], |row| {
            row.get(0)
        })?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesSessionReservationCurrentV2 {
            version: 2,
            snapshot,
            last_event_seq: seq,
            last_event_hash: hash,
        })
    }
    pub(crate) fn lookup_kubernetes_session_recovery_receipt(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionRecoveryReceiptLookupV2,
        close: &KubernetesSessionReservationCloseV2,
        policy: &KubernetesSessionRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionReservationMutationResponseV2> {
        close.validate()?;
        let Basis::RecoveryReceiptLookup {
            target_hash,
            close_seq,
            close_hash,
            policy_hash,
            ..
        } = &authorization.evidence.basis
        else {
            return Err(Failure::PermissionDenied.into());
        };
        permission(*target_hash == close.input_hash()? && *policy_hash == policy.policy_hash()?)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let (snapshot, _) = replay::load_snapshot(&tx, &close.reservation_id, &self.tenant_id)?
            .ok_or(Failure::NotFound)?;
        current_evidence(
            &tx,
            identity,
            &authorization.evidence,
            &snapshot.intent,
            &self.tenant_id,
            clock.at(),
        )?;
        conflict(
            snapshot.state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect
                && snapshot.state.last_close.as_ref() == Some(close),
        )?;
        let (stored, record) =
            replay::operation(&tx, &close.operation_id)?.ok_or(Failure::NotFound)?;
        conflict(
            stored.seq == *close_seq
                && stored.hash == *close_hash
                && record.intent.same(&snapshot.intent)?
                && record.change
                    == Change::CloseBeforeEffect {
                        close: close.clone(),
                    },
        )?;
        replay::validate_historical(&tx, &stored, &record, &self.tenant_id)?;
        let response = KubernetesSessionReservationMutationResponseV2 {
            version: 2,
            receipt: model::receipt(&stored, &record)?,
            snapshot,
            replayed: true,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(response)
    }
}
