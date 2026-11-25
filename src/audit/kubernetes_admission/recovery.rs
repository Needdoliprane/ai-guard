//! Exact no-effect recovery. None of these proofs can authorize runtime work.
use super::*;
use crate::broker_registry::{
    BrokerIdentityV1, BrokerRuntimeKindV1, BrokerV1, format_time, parse_time,
};
use crate::control::QuotaPolicyV1;
use crate::workload::WorkloadBinding;
use authorization::{AuthorizationEvidence, AuthorizationEvidenceV3, RecoveryBasisV3, permission};
use chrono::{DateTime, Duration, Utc};
use model::{Change, IdentityEvidence, Record, conflict};

#[path = "recovery_history.rs"]
mod history;

pub(super) fn validate_historical_identity(
    db: &Connection,
    stored: &StoredEvent,
    record: &Record,
    tenant: &str,
    evidence: &AuthorizationEvidenceV3,
) -> anyhow::Result<()> {
    history::validate_historical_identity(db, stored, record, tenant, evidence)
}

/// Only checked Store preparation constructs either variant. A JSON request
/// cannot choose Historical, synthesize a proof, or reconstruct this enum.
// Recovery authorization value contract; keep the by-value shape stable.
#[allow(clippy::large_enum_variant)]
pub(crate) enum PreparedRecovery {
    Historical(KubernetesCreateRecoveryLookupAuthorization),
    New(KubernetesCreateRecoveryAuthorization),
}
pub(crate) struct KubernetesCreateRecoveryAuthorization {
    request: KubernetesCreateRecoveryV3,
    evidence: AuthorizationEvidenceV3,
}
pub(crate) struct KubernetesCreateRecoveryLookupAuthorization {
    request: KubernetesCreateRecoveryV3,
    actor: Actor,
    reservation_seq: i64,
    reservation_hash: String,
    close_seq: i64,
    close_hash: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}
struct Actor {
    tenant_id: String,
    broker_id: String,
    incarnation_id: String,
    credential_id: String,
    binding_revision: u64,
    grant_revision: u64,
}
impl Actor {
    fn capture(identity: &BrokerIdentityV1, broker: &BrokerV1) -> Self {
        Self {
            tenant_id: identity.tenant_id().into(),
            broker_id: identity.broker_id().into(),
            incarnation_id: broker.incarnation_id.clone(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        }
    }
    fn validate(&self, identity: &BrokerIdentityV1, broker: &BrokerV1) -> anyhow::Result<()> {
        permission(
            self.tenant_id == identity.tenant_id()
                && self.broker_id == identity.broker_id()
                && self.incarnation_id == broker.incarnation_id
                && self.credential_id == identity.credential_id()
                && self.binding_revision == identity.binding_revision()
                && self.grant_revision == identity.grant_revision(),
        )
    }
}
pub(super) struct CurrentScope {
    broker: BrokerV1,
    binding: WorkloadBinding,
}
impl CurrentScope {
    fn expires(&self, at: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
        let expires = at
            .checked_add_signed(Duration::seconds(60))
            .ok_or(KubernetesCreateFailureV1::Unavailable)?
            .min(parse_time(&self.broker.not_after)?)
            .min(parse_time(&self.binding.expires_at)?);
        permission(at < expires)?;
        Ok(expires)
    }
}

fn require_recovery_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 13 {
        return Err(KubernetesCreateFailureV1::MigrationRequired.into());
    }
    migration::validate_recovery_marker(db)
}

fn current_scope(
    db: &Connection,
    identity: &BrokerIdentityV1,
    intent: &KubernetesCreateIntentV1,
    at: DateTime<Utc>,
) -> anyhow::Result<CurrentScope> {
    workload::revalidate_broker_identity(db, identity, at)?;
    let broker: BrokerV1 = serde_json::from_str(&db.query_row(
        "SELECT payload_json FROM broker_registry_brokers WHERE broker_id=?1",
        [identity.broker_id()],
        |row| row.get::<_, String>(0),
    )?)?;
    let binding: WorkloadBinding = serde_json::from_str(&db.query_row(
        "SELECT payload_json FROM workload_bindings WHERE credential_id=?1",
        [identity.credential_id()],
        |row| row.get::<_, String>(0),
    )?)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    permission(
        namespace == intent.control_namespace_uuid
            && intent.broker_id == identity.broker_id()
            && broker.authorities.iter().any(|authority| {
                authority.authority_id == intent.authority.authority_id().to_string()
                    && authority.runtime_kind == BrokerRuntimeKindV1::Kubernetes
            }),
    )?;
    Ok(CurrentScope { broker, binding })
}

pub(super) fn validate_evidence(
    value: &AuthorizationEvidenceV3,
    identity: &BrokerIdentityV1,
    intent: &KubernetesCreateIntentV1,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    value.quota.validate()?;
    let issued = parse_time(&value.issued_at)?;
    let expires = parse_time(&value.expires_at)?;
    let RecoveryBasisV3::RecoverBeforeEmission {
        reservation_seq,
        reservation_hash,
        admission_decision_hash,
        creator_incarnation_id,
        reserved_snapshot_hash,
        recovery_input_hash,
    } = &value.basis;
    for digest in [
        &value.origin_hash,
        &value.authority_fingerprint,
        &value.intent_hash,
        &value.decision_hash,
        reservation_hash,
        admission_decision_hash,
        reserved_snapshot_hash,
        recovery_input_hash,
    ] {
        crate::control::hash(digest)?;
    }
    crate::control::uuid(&value.incarnation_id)?;
    permission(!uuid::Uuid::parse_str(&value.incarnation_id)?.is_nil())?;
    permission(
        value.version == 3
            && value.tenant_id == identity.tenant_id()
            && value.broker_id == identity.broker_id()
            && value.broker_id == intent.broker_id
            && value.credential_id == identity.credential_id()
            && value.binding_revision == identity.binding_revision()
            && value.grant_revision == identity.grant_revision()
            && value.control_namespace_uuid == intent.control_namespace_uuid
            && value.authority_fingerprint == intent.authority.fingerprint()
            && value.intent_hash == intent.intent_hash()?
            && *creator_incarnation_id == intent.incarnation_id
            && *reservation_seq > 0
            && *reservation_seq <= value.origin_seq
            && format_time(issued) == value.issued_at
            && format_time(expires) == value.expires_at
            && issued <= at
            && at < expires
            && expires
                <= issued
                    .checked_add_signed(Duration::seconds(60))
                    .ok_or(KubernetesCreateFailureV1::Unavailable)?
            && value.decision_hash == value.commitment()?,
    )
}

struct Entry {
    stored: StoredEvent,
    record: Record,
}
impl Entry {
    fn request(&self) -> anyhow::Result<&KubernetesCreateRecoveryV3> {
        match &self.record.change {
            Change::RecoverBeforeEmission { recovery } => Ok(recovery),
            _ => Err(KubernetesCreateFailureV1::Conflict.into()),
        }
    }
    fn result(&self) -> anyhow::Result<KubernetesCreateRecoveryResultV3> {
        Ok(KubernetesCreateRecoveryResultV3 {
            version: 3,
            recovery: self.request()?.clone(),
            receipt: model::receipt(&self.stored, &self.record),
            snapshot: self.record.snapshot.clone(),
            replayed: true,
        })
    }
}
fn entry(
    db: &Connection,
    tenant: &str,
    create_id: &str,
    operation_id: &str,
    request: Option<&KubernetesCreateRecoveryV3>,
) -> anyhow::Result<Option<Entry>> {
    let Some(stored) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [operation_id],
            row_event,
        )
        .optional()?
    else {
        return Ok(None);
    };
    conflict(stored.event.kind == "control.kubernetes.create.recovered_before_emission")?;
    let record = replay::record(&stored)?;
    let result = Entry { stored, record };
    conflict(
        result.record.version == 3
            && result.request()?.create_id == create_id
            && result.request()?.operation_id == operation_id
            && request
                .is_none_or(|expected| result.request().is_ok_and(|actual| actual == expected)),
    )?;
    replay::validate_historical_identity(db, &result.stored, &result.record, tenant)?;
    Ok(Some(result))
}

fn initial(
    db: &Connection,
    tenant: &str,
    snapshot: &KubernetesCreateSnapshotV1,
) -> anyhow::Result<(StoredEvent, Record)> {
    let (stored, record) = replay::reservation(db, &snapshot.intent, tenant)?;
    let expected = model::transition(
        &snapshot.intent,
        None,
        &Change::Reserve {
            intent: snapshot.intent.clone(),
        },
        None,
    )?;
    conflict(record.snapshot == expected && *snapshot == expected)?;
    Ok((stored, record))
}

fn lookup_proof(
    identity: &BrokerIdentityV1,
    scope: &CurrentScope,
    old: &Entry,
    at: DateTime<Utc>,
) -> anyhow::Result<KubernetesCreateRecoveryLookupAuthorization> {
    let AuthorizationEvidence::V3(evidence) = &old.record.authorization else {
        return Err(KubernetesCreateFailureV1::Conflict.into());
    };
    let RecoveryBasisV3::RecoverBeforeEmission {
        reservation_seq,
        reservation_hash,
        ..
    } = &evidence.basis;
    Ok(KubernetesCreateRecoveryLookupAuthorization {
        request: old.request()?.clone(),
        actor: Actor::capture(identity, &scope.broker),
        reservation_seq: *reservation_seq,
        reservation_hash: reservation_hash.clone(),
        close_seq: old.stored.seq,
        close_hash: old.stored.hash.clone(),
        issued_at: at,
        expires_at: scope.expires(at)?,
    })
}

impl Store {
    pub(crate) fn prepare_kubernetes_create_recovery(
        &self,
        identity: &BrokerIdentityV1,
        request: &KubernetesCreateRecoveryV3,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<PreparedRecovery> {
        request.validate()?;
        quota.validate()?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let input = serde_json::to_value(request)?;
        permission(self.redactor.value(&input) == input)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_recovery_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        // Authentication precedes every lookup, including successful old UUIDs.
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        let snapshot = replay::load_snapshot(&tx, &request.create_id)?
            .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let scope = current_scope(&tx, identity, &snapshot.intent, clock.at())?;
        conflict(request.intent_hash == snapshot.intent_hash)?;
        if let Some(old) = entry(
            &tx,
            &self.tenant_id,
            &request.create_id,
            &request.operation_id,
            Some(request),
        )? {
            let proof = lookup_proof(identity, &scope, &old, clock.at())?;
            self.commit_authorized(tx, &clock)?;
            return Ok(PreparedRecovery::Historical(proof));
        }
        let (reserved, original) = initial(&tx, &self.tenant_id, &snapshot)?;
        let (origin_seq, origin_hash) = audit_head(&tx)?;
        let mut evidence = AuthorizationEvidenceV3 {
            version: 3,
            tenant_id: self.tenant_id.clone(),
            control_namespace_uuid: snapshot.intent.control_namespace_uuid.clone(),
            broker_id: identity.broker_id().into(),
            incarnation_id: scope.broker.incarnation_id.clone(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
            origin_seq,
            origin_hash,
            authority_fingerprint: snapshot.intent.authority.fingerprint(),
            intent_hash: snapshot.intent_hash.clone(),
            quota: quota.clone(),
            issued_at: format_time(clock.at()),
            expires_at: format_time(scope.expires(clock.at())?),
            decision_hash: String::new(),
            basis: RecoveryBasisV3::RecoverBeforeEmission {
                reservation_seq: reserved.seq,
                reservation_hash: reserved.hash,
                admission_decision_hash: original.authorization.decision_hash().clone(),
                creator_incarnation_id: snapshot.intent.incarnation_id.clone(),
                reserved_snapshot_hash: original.snapshot.snapshot_hash()?,
                recovery_input_hash: request.input_hash()?,
            },
        };
        evidence.decision_hash = evidence.commitment()?;
        validate_evidence(&evidence, identity, &snapshot.intent, clock.at())?;
        history::validate_basis(&tx, &evidence, &snapshot.intent, &self.tenant_id, None)?;
        self.commit_authorized(tx, &clock)?;
        Ok(PreparedRecovery::New(
            KubernetesCreateRecoveryAuthorization {
                request: request.clone(),
                evidence,
            },
        ))
    }

    pub(crate) fn recover_kubernetes_create(
        &self,
        identity: &BrokerIdentityV1,
        proof: &KubernetesCreateRecoveryAuthorization,
    ) -> anyhow::Result<KubernetesCreateRecoveryResultV3> {
        proof.request.validate()?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_recovery_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        let snapshot = replay::load_snapshot(&tx, &proof.request.create_id)?
            .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let scope = current_scope(&tx, identity, &snapshot.intent, clock.at())?;
        validate_evidence(&proof.evidence, identity, &snapshot.intent, clock.at())?;
        permission(proof.evidence.incarnation_id == scope.broker.incarnation_id)?;
        history::validate_basis(
            &tx,
            &proof.evidence,
            &snapshot.intent,
            &self.tenant_id,
            None,
        )?;
        let RecoveryBasisV3::RecoverBeforeEmission {
            recovery_input_hash,
            ..
        } = &proof.evidence.basis;
        permission(proof.request.input_hash()? == *recovery_input_hash)?;
        // A concurrent exact commit or lost ACK must be recoverable even though
        // the materialized state is no longer Reserved.
        if let Some(old) = entry(
            &tx,
            &self.tenant_id,
            &proof.request.create_id,
            &proof.request.operation_id,
            Some(&proof.request),
        )? {
            let result = old.result()?;
            self.commit_authorized(tx, &clock)?;
            return Ok(result);
        }
        initial(&tx, &self.tenant_id, &snapshot)?;
        let change = Change::RecoverBeforeEmission {
            recovery: proof.request.clone(),
        };
        let next = model::transition(&snapshot.intent, Some(&snapshot), &change, None)?;
        let record = Record {
            version: 3,
            input_hash: proof.request.input_hash()?,
            received_at: format_time(clock.at()),
            identity: IdentityEvidence::capture(identity),
            authorization: AuthorizationEvidence::V3(proof.evidence.clone()),
            intent: snapshot.intent.clone(),
            previous_revision: snapshot.revision,
            change,
            snapshot: next,
        };
        let mut event = Event::new(record.change.kind());
        event.id = proof.request.operation_id.clone();
        event.timestamp = record.received_at.clone();
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
        replay::write_snapshot(&tx, &record.snapshot, stored.seq, Some(snapshot.revision))?;
        let result = KubernetesCreateRecoveryResultV3 {
            version: 3,
            recovery: proof.request.clone(),
            receipt: model::receipt(&stored, &record),
            snapshot: record.snapshot,
            replayed: false,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }

    pub(crate) fn authorize_kubernetes_create_recovery_lookup(
        &self,
        identity: &BrokerIdentityV1,
        create_id: &str,
        operation_id: &str,
    ) -> anyhow::Result<KubernetesCreateRecoveryLookupAuthorization> {
        crate::control::uuid(create_id)?;
        crate::control::uuid(operation_id)?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_recovery_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        let snapshot =
            replay::load_snapshot(&tx, create_id)?.ok_or(KubernetesCreateFailureV1::NotFound)?;
        let scope = current_scope(&tx, identity, &snapshot.intent, clock.at())?;
        let old = entry(&tx, &self.tenant_id, create_id, operation_id, None)?
            .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let proof = lookup_proof(identity, &scope, &old, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(proof)
    }

    pub(crate) fn lookup_kubernetes_create_recovery(
        &self,
        identity: &BrokerIdentityV1,
        proof: &KubernetesCreateRecoveryLookupAuthorization,
    ) -> anyhow::Result<KubernetesCreateRecoveryResultV3> {
        permission(identity.tenant_id() == self.tenant_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_recovery_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        let snapshot = replay::load_snapshot(&tx, &proof.request.create_id)?
            .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let scope = current_scope(&tx, identity, &snapshot.intent, clock.at())?;
        proof.actor.validate(identity, &scope.broker)?;
        permission(proof.issued_at <= clock.at() && clock.at() < proof.expires_at)?;
        let old = entry(
            &tx,
            &self.tenant_id,
            &proof.request.create_id,
            &proof.request.operation_id,
            Some(&proof.request),
        )?
        .ok_or(KubernetesCreateFailureV1::NotFound)?;
        let AuthorizationEvidence::V3(evidence) = &old.record.authorization else {
            return Err(KubernetesCreateFailureV1::Conflict.into());
        };
        let RecoveryBasisV3::RecoverBeforeEmission {
            reservation_seq,
            reservation_hash,
            ..
        } = &evidence.basis;
        conflict(
            old.stored.seq == proof.close_seq
                && old.stored.hash == proof.close_hash
                && *reservation_seq == proof.reservation_seq
                && *reservation_hash == proof.reservation_hash,
        )?;
        let result = old.result()?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }
}
