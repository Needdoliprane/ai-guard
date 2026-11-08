use super::*;
use model::{Change, Creator, IdentityEvidence};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Observe,
    CloseBeforeEffect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RecoveryAction {
    ObserveReserved,
    CloseReserved,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Basis {
    Reserve {
        source: PolicySource,
        policy_snapshot: KubernetesSessionReservationPolicyV2,
        policy_hash: String,
    },
    Reconcile {
        action: Action,
        target_hash: String,
        reservation_seq: i64,
        reservation_hash: String,
        reservation_decision_hash: String,
    },
    RecoverBeforeEffect {
        action: RecoveryAction,
        target_hash: String,
        creator_reference: Creator,
        policy_snapshot: KubernetesSessionRecoveryPolicyV2,
        policy_hash: String,
    },
    RecoveryReceiptLookup {
        target_hash: String,
        creator_reference: Creator,
        close_seq: i64,
        close_hash: String,
        policy_snapshot: KubernetesSessionRecoveryPolicyV2,
        policy_hash: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PolicySource {
    TenantConfiguration,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    pub version: u32,
    pub actor: IdentityEvidence,
    pub control_namespace_uuid: String,
    pub origin_seq: i64,
    pub origin_hash: String,
    pub authority_fingerprint: String,
    pub reservation_input_hash: String,
    pub session_hash: String,
    pub attempt_hash: String,
    pub profile_hash: String,
    pub quota: crate::control::QuotaPolicyV1,
    pub issued_at: String,
    pub expires_at: String,
    pub basis: Basis,
    pub decision_hash: String,
}
impl Evidence {
    pub(super) fn commitment(&self) -> anyhow::Result<String> {
        let a = &self.actor;
        match &self.basis {
            Basis::RecoverBeforeEffect {
                action,
                target_hash,
                creator_reference,
                policy_snapshot,
                policy_hash,
            } => {
                let Creator::Reservation {
                    reservation_seq,
                    reservation_hash,
                    ..
                } = creator_reference
                else {
                    return Err(Failure::InvalidRequest.into());
                };
                digest(
                    b"ai-guard:kubernetes-session-recover-before-effect:v2\0",
                    &json!([
                    2, a.tenant_id, self.control_namespace_uuid, a, creator_reference, reservation_seq, reservation_hash,
                    self.reservation_input_hash, self.session_hash, self.attempt_hash, self.profile_hash,
                    self.origin_seq, self.origin_hash, "reserved", 1, {"state":"never_issued","revision":0},
                    action, target_hash, policy_snapshot, policy_hash, self.issued_at, self.expires_at]),
                )
            }
            Basis::RecoveryReceiptLookup {
                target_hash,
                creator_reference,
                close_seq,
                close_hash,
                policy_hash,
                ..
            } => digest(
                b"ai-guard:kubernetes-session-recovery-receipt:v2\0",
                &json!([
                    2,
                    a.tenant_id,
                    self.control_namespace_uuid,
                    a,
                    creator_reference,
                    self.reservation_input_hash,
                    target_hash,
                    close_seq,
                    close_hash,
                    self.origin_seq,
                    self.origin_hash,
                    policy_hash,
                    self.issued_at,
                    self.expires_at
                ]),
            ),
            _ => digest(
                b"ai-guard:kubernetes-session-reservation-authorization:v2\0",
                &json!([
                    self.version,
                    a.tenant_id,
                    self.control_namespace_uuid,
                    a.broker_id,
                    a.incarnation_id,
                    a.credential_id,
                    a.binding_revision,
                    a.grant_revision,
                    self.origin_seq,
                    self.origin_hash,
                    self.authority_fingerprint,
                    self.reservation_input_hash,
                    self.session_hash,
                    self.attempt_hash,
                    self.profile_hash,
                    self.quota,
                    self.issued_at,
                    self.expires_at,
                    self.basis
                ]),
            ),
        }
    }
    pub(super) fn validate(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
        actor: &IdentityEvidence,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        actor.identity()?;
        self.quota.validate()?;
        let ids = intent.ids()?;
        permission(
            self.version == 2
                && self.actor == *actor
                && self.origin_seq > 0
                && self.control_namespace_uuid == ids.control_namespace_uuid.to_string()
                && self.actor.broker_id == ids.broker_id.to_string()
                && self.authority_fingerprint == intent.authority().fingerprint()
                && self.reservation_input_hash == intent.input_hash(&actor.tenant_id)?
                && self.session_hash == intent.session_hash(&actor.tenant_id)?
                && self.attempt_hash == intent.attempt_hash(&actor.tenant_id)?
                && self.profile_hash == intent.profile_hash()?
                && self.decision_hash == self.commitment()?
                && self.issued_at.len() == 24
                && self.expires_at.len() == 24
                && format_time(parse_time(&self.issued_at)?) == self.issued_at
                && format_time(parse_time(&self.expires_at)?) == self.expires_at
                && parse_time(&self.issued_at)? <= at
                && at < parse_time(&self.expires_at)?,
        )?;
        hash(&self.origin_hash)?;
        let maximum = match &self.basis {
            Basis::Reserve {
                policy_snapshot,
                policy_hash,
                ..
            } => {
                permission(
                    policy_snapshot == &policy_snapshot.canonicalized()?
                        && *policy_hash == policy_snapshot.policy_hash()?
                        && policy_snapshot.approves(intent)?
                        && actor.incarnation_id == ids.incarnation_id.to_string(),
                )?;
                policy_snapshot.decision_ttl_seconds
            }
            Basis::Reconcile {
                target_hash,
                reservation_seq,
                reservation_hash,
                reservation_decision_hash,
                ..
            } => {
                hash(target_hash)?;
                hash(reservation_hash)?;
                hash(reservation_decision_hash)?;
                permission(
                    *reservation_seq > 0
                        && *reservation_seq <= self.origin_seq
                        && actor.incarnation_id == ids.incarnation_id.to_string(),
                )?;
                60
            }
            Basis::RecoverBeforeEffect {
                creator_reference,
                target_hash,
                policy_snapshot,
                policy_hash,
                ..
            }
            | Basis::RecoveryReceiptLookup {
                creator_reference,
                target_hash,
                policy_snapshot,
                policy_hash,
                ..
            } => {
                let Creator::Reservation {
                    reservation_seq,
                    reservation_hash,
                    actor: creator,
                } = creator_reference
                else {
                    return Err(Failure::PermissionDenied.into());
                };
                creator.identity()?;
                hash(reservation_hash)?;
                hash(target_hash)?;
                permission(
                    *reservation_seq > 0
                        && *reservation_seq <= self.origin_seq
                        && creator.tenant_id == actor.tenant_id
                        && creator.broker_id == actor.broker_id
                        && creator.incarnation_id == ids.incarnation_id.to_string()
                        && creator.incarnation_id != actor.incarnation_id
                        && creator.grant_revision < actor.grant_revision
                        && policy_snapshot == &policy_snapshot.canonicalized()?
                        && *policy_hash == policy_snapshot.policy_hash()?
                        && policy_snapshot.approves(&actor.broker_id)?,
                )?;
                if let Basis::RecoveryReceiptLookup {
                    close_seq,
                    close_hash,
                    ..
                } = &self.basis
                {
                    permission(*close_seq > *reservation_seq && *close_seq <= self.origin_seq)?;
                    hash(close_hash)?;
                }
                policy_snapshot.decision_ttl_seconds
            }
        };
        permission(
            parse_time(&self.expires_at)?
                <= parse_time(&self.issued_at)?
                    .checked_add_signed(chrono::Duration::seconds(i64::from(maximum)))
                    .ok_or(Failure::Unavailable)?,
        )
    }
    pub(super) fn allows(
        &self,
        change: &Change,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> anyhow::Result<()> {
        let input_hash = change.input_hash(intent, &self.actor.tenant_id)?;
        permission(match (&self.basis, change) {
            (Basis::Reserve { .. }, Change::Reserve) => true,
            (
                Basis::Reconcile {
                    action: Action::CloseBeforeEffect,
                    target_hash,
                    ..
                },
                Change::CloseBeforeEffect { .. },
            )
            | (
                Basis::RecoverBeforeEffect {
                    action: RecoveryAction::CloseReserved,
                    target_hash,
                    ..
                },
                Change::CloseBeforeEffect { .. },
            ) => *target_hash == input_hash,
            _ => false,
        })
    }
}

/// Checked Store issuers only. Descriptive evidence never recreates authority.
pub(crate) struct KubernetesSessionReservationAuthorizationV2 {
    pub(super) evidence: Evidence,
}
pub(crate) struct KubernetesSessionRecoverBeforeEffectV2 {
    pub(super) evidence: Evidence,
}
pub(crate) struct KubernetesSessionRecoveryReceiptLookupV2 {
    pub(super) evidence: Evidence,
}
pub(crate) enum KubernetesSessionRecoveryAuthorizationV2 {
    BeforeEffect(KubernetesSessionRecoverBeforeEffectV2),
    ReceiptLookup(KubernetesSessionRecoveryReceiptLookupV2),
}

pub(super) fn current(
    db: &Connection,
    identity: &BrokerIdentityV1,
    intent: &KubernetesSessionReservationIntentV2,
    tenant: &str,
    successor: bool,
    at: DateTime<Utc>,
) -> anyhow::Result<BrokerV1> {
    workload::revalidate_broker_identity(db, identity, at)?;
    let ids = intent.ids()?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let broker: BrokerV1 = serde_json::from_str(&db.query_row(
        "SELECT payload_json FROM broker_registry_brokers WHERE broker_id=?1",
        [identity.broker_id()],
        |row| row.get::<_, String>(0),
    )?)?;
    permission(
        identity.tenant_id() == tenant
            && namespace == ids.control_namespace_uuid.to_string()
            && identity.broker_id() == ids.broker_id.to_string(),
    )?;
    if !successor {
        permission(
            broker.incarnation_id == ids.incarnation_id.to_string()
                && broker.authorities.iter().any(|authority| {
                    authority.runtime_kind == BrokerRuntimeKindV1::Kubernetes
                        && authority.authority_id == intent.authority().authority_id().to_string()
                }),
        )?;
    }
    Ok(broker)
}
pub(super) fn evidence(
    db: &Connection,
    identity: &BrokerIdentityV1,
    broker: &BrokerV1,
    intent: &KubernetesSessionReservationIntentV2,
    quota: &crate::control::QuotaPolicyV1,
    basis: Basis,
    at: DateTime<Utc>,
) -> anyhow::Result<Evidence> {
    let binding = workload::prefix_binding(
        db,
        identity.credential_id(),
        audit_head(db)?
            .0
            .checked_add(1)
            .ok_or(Failure::Unavailable)?,
    )?
    .ok_or(Failure::PermissionDenied)?;
    let ttl = match &basis {
        Basis::Reserve {
            policy_snapshot, ..
        } => policy_snapshot.decision_ttl_seconds,
        Basis::RecoverBeforeEffect {
            policy_snapshot, ..
        }
        | Basis::RecoveryReceiptLookup {
            policy_snapshot, ..
        } => policy_snapshot.decision_ttl_seconds,
        Basis::Reconcile { .. } => 60,
    };
    let expires = at
        .checked_add_signed(chrono::Duration::seconds(i64::from(ttl)))
        .ok_or(Failure::Unavailable)?
        .min(parse_time(&binding.expires_at)?)
        .min(parse_time(&broker.not_after)?);
    permission(at < expires)?;
    let (origin_seq, origin_hash) = audit_head(db)?;
    let ids = intent.ids()?;
    let mut evidence = Evidence {
        version: 2,
        actor: IdentityEvidence::capture(identity, broker),
        control_namespace_uuid: ids.control_namespace_uuid.to_string(),
        origin_seq,
        origin_hash,
        authority_fingerprint: intent.authority().fingerprint(),
        reservation_input_hash: intent.input_hash(identity.tenant_id())?,
        session_hash: intent.session_hash(identity.tenant_id())?,
        attempt_hash: intent.attempt_hash(identity.tenant_id())?,
        profile_hash: intent.profile_hash()?,
        quota: quota.clone(),
        issued_at: format_time(at),
        expires_at: format_time(expires),
        basis,
        decision_hash: String::new(),
    };
    evidence.decision_hash = evidence.commitment()?;
    evidence.validate(intent, &evidence.actor, at)?;
    replay::validate_origin(db, &evidence, None)?;
    Ok(evidence)
}

impl Store {
    pub(crate) fn authorize_kubernetes_session_reservation(
        &self,
        identity: &BrokerIdentityV1,
        intent: &KubernetesSessionReservationIntentV2,
        policy: &KubernetesSessionReservationPolicyV2,
        quota: &crate::control::QuotaPolicyV1,
    ) -> anyhow::Result<KubernetesSessionReservationAuthorizationV2> {
        intent.validate()?;
        quota.validate()?;
        let policy = policy.canonicalized()?;
        permission(policy.approves(intent)?)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let broker = current(&tx, identity, intent, &self.tenant_id, false, clock.at())?;
        if !tx.lease.active {
            return Err(Failure::SupervisionInactive.into());
        }
        let basis = Basis::Reserve {
            source: PolicySource::TenantConfiguration,
            policy_hash: policy.policy_hash()?,
            policy_snapshot: policy,
        };
        let evidence = evidence(&tx, identity, &broker, intent, quota, basis, clock.at())?;
        mutations::preflight(self, &tx, &evidence, intent, &Change::Reserve, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesSessionReservationAuthorizationV2 { evidence })
    }
    pub(crate) fn authorize_kubernetes_session_reconciliation(
        &self,
        identity: &BrokerIdentityV1,
        reservation_id: &str,
        close: Option<&KubernetesSessionReservationCloseV2>,
    ) -> anyhow::Result<KubernetesSessionReservationAuthorizationV2> {
        uuid(reservation_id)?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let (stored, original) = replay::reservation(&tx, reservation_id, &self.tenant_id)?;
        let broker = current(
            &tx,
            identity,
            &original.intent,
            &self.tenant_id,
            false,
            clock.at(),
        )?;
        let target_hash = match close {
            Some(close) => {
                permission(
                    close.reservation_id == reservation_id
                        && close.reservation_input_hash
                            == original.intent.input_hash(&self.tenant_id)?,
                )?;
                close.input_hash()?
            }
            None => original.intent.input_hash(&self.tenant_id)?,
        };
        let basis = Basis::Reconcile {
            action: if close.is_some() {
                Action::CloseBeforeEffect
            } else {
                Action::Observe
            },
            target_hash,
            reservation_seq: stored.seq,
            reservation_hash: stored.hash,
            reservation_decision_hash: original.authorization.decision_hash.clone(),
        };
        let evidence = evidence(
            &tx,
            identity,
            &broker,
            &original.intent,
            &original.authorization.quota,
            basis,
            clock.at(),
        )?;
        if let Some(close) = close {
            mutations::preflight(
                self,
                &tx,
                &evidence,
                &original.intent,
                &Change::CloseBeforeEffect {
                    close: close.clone(),
                },
                clock.at(),
            )?;
        }
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesSessionReservationAuthorizationV2 { evidence })
    }
    pub(crate) fn authorize_kubernetes_session_recovery(
        &self,
        identity: &BrokerIdentityV1,
        reservation_id: &str,
        close: Option<&KubernetesSessionReservationCloseV2>,
        policy: &KubernetesSessionRecoveryPolicyV2,
    ) -> anyhow::Result<KubernetesSessionRecoveryAuthorizationV2> {
        uuid(reservation_id)?;
        let policy = policy.canonicalized()?;
        let mut db = self.db.lock().map_err(|_| Failure::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(
            identity.tenant_id() == self.tenant_id && policy.approves(identity.broker_id())?,
        )?;
        let (stored, original) = replay::reservation(&tx, reservation_id, &self.tenant_id)?;
        let broker = current(
            &tx,
            identity,
            &original.intent,
            &self.tenant_id,
            true,
            clock.at(),
        )?;
        let creator = Creator::Reservation {
            reservation_seq: stored.seq,
            reservation_hash: stored.hash,
            actor: original.actor.identity(),
        };
        let current = replay::load_snapshot(&tx, reservation_id, &self.tenant_id)?
            .ok_or(Failure::NotFound)?;
        let target_hash = match close {
            Some(close) => {
                permission(
                    close.reservation_id == reservation_id
                        && close.reservation_input_hash
                            == original.intent.input_hash(&self.tenant_id)?,
                )?;
                close.input_hash()?
            }
            None => original.intent.input_hash(&self.tenant_id)?,
        };
        let closed =
            current.0.state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect;
        let basis = if closed {
            let close = close.ok_or(Failure::Conflict)?;
            let (close_stored, close_record) =
                replay::operation(&tx, &close.operation_id)?.ok_or(Failure::NotFound)?;
            conflict(
                matches!(&close_record.change, Change::CloseBeforeEffect { close: exact } if exact == close)
                    && close_record.intent.same(&original.intent)?,
            )?;
            replay::validate_historical(&tx, &close_stored, &close_record, &self.tenant_id)?;
            Basis::RecoveryReceiptLookup {
                target_hash,
                creator_reference: creator,
                close_seq: close_stored.seq,
                close_hash: close_stored.hash,
                policy_hash: policy.policy_hash()?,
                policy_snapshot: policy,
            }
        } else {
            Basis::RecoverBeforeEffect {
                action: if close.is_some() {
                    RecoveryAction::CloseReserved
                } else {
                    RecoveryAction::ObserveReserved
                },
                target_hash,
                creator_reference: creator,
                policy_hash: policy.policy_hash()?,
                policy_snapshot: policy,
            }
        };
        let evidence = evidence(
            &tx,
            identity,
            &broker,
            &original.intent,
            &original.authorization.quota,
            basis,
            clock.at(),
        )?;
        if let Some(close) = close
            && !closed
        {
            mutations::preflight(
                self,
                &tx,
                &evidence,
                &original.intent,
                &Change::CloseBeforeEffect {
                    close: close.clone(),
                },
                clock.at(),
            )?;
        }
        self.commit_authorized(tx, &clock)?;
        Ok(if closed {
            KubernetesSessionRecoveryAuthorizationV2::ReceiptLookup(
                KubernetesSessionRecoveryReceiptLookupV2 { evidence },
            )
        } else {
            KubernetesSessionRecoveryAuthorizationV2::BeforeEffect(
                KubernetesSessionRecoverBeforeEffectV2 { evidence },
            )
        })
    }
}
