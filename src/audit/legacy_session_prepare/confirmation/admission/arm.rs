//! Historical validation of the first Legacy named Import arm.
use super::*;
use crate::control::{
    ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1, WorkerStateV1, WorkerV1,
};

pub(in crate::audit) const LEGACY_NAMED_ARM_KIND: &str = "control.execution.arm.legacy_named";

/// An inspected helper and its exact retained admission. A claim requires
/// separate authorization. Safe closure requires independent effect evidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedArmV1 {
    pub(in crate::audit) version: u32,
    pub(in crate::audit) binding_ref: Uuid,
    pub(in crate::audit) admission_fact_seq: i64,
    pub(in crate::audit) admission_fact_hash: String,
    pub(in crate::audit) namespace: String,
    pub(in crate::audit) session_id: String,
    pub(in crate::audit) authorization_revision: u64,
    pub(in crate::audit) authorized_at: DateTime<Utc>,
    pub(in crate::audit) arm: ArmV1,
}

impl LegacyNamedArmV1 {
    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::invalid(
            self.version == 1
                && !self.binding_ref.is_nil()
                && self.admission_fact_seq > 0
                && workload::canonical_control_time(self.authorized_at)? == self.authorized_at
                && matches!(
                    &self.arm.subject,
                    ArmSubjectV1::Helper {
                        purpose: HelperPurposeV1::Import,
                        ..
                    }
                ),
        )?;
        canonical_uuid(&self.namespace)?;
        canonical_uuid(&self.session_id)?;
        crate::control::hash(&self.admission_fact_hash)?;
        self.arm.validate()?;
        crate::control::content_hash("ai-guard:legacy-named-import-arm:v1", self)
    }
}

fn arm_admission(
    db: &Connection,
    arm: &LegacyNamedArmV1,
    before_seq: i64,
) -> anyhow::Result<AdmissionFact> {
    let stored = event_by_id(db, &fact_id(arm.binding_ref))?.ok_or(ControlFailure::NotFound)?;
    let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
    conflict(
        fact.version == 1
            && stored.seq == arm.admission_fact_seq
            && stored.hash == arm.admission_fact_hash
            && stored.seq < before_seq
            && fact.binding_ref == arm.binding_ref
            && fact.admission.audit_seq < stored.seq
            && fact
                .grant
                .as_ref()
                .is_some_and(|grant| grant.audit_seq < stored.seq),
    )?;
    Ok(fact)
}

pub(in crate::audit) fn legacy_named_arm_predecessor(
    db: &Connection,
    arm: &LegacyNamedArmV1,
    parent: &WorkerV1,
    before_seq: i64,
) -> anyhow::Result<()> {
    let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    conflict(format_admits(version))?;
    let fact = arm_admission(db, arm, before_seq)?;
    let (_, record) = admission_record(db, &fact)?;
    let (request, worker_record) = record.admission()?;
    let admitted = worker_record
        .worker
        .as_ref()
        .ok_or(ControlFailure::NotFound)?;
    let (confirmed, confirmation, prepared) = admission_origin(db, arm.binding_ref)?;
    let authority = confirmation
        .admission_authority
        .as_ref()
        .filter(|_| confirmation.version == 2)
        .ok_or(ControlFailure::NotFound)?;
    let (target_event, target) = require_target_at(
        db,
        confirmation.request.selection()?,
        &prepared.initiator,
        before_seq,
    )?;
    conflict(
        fact.confirmation_hash == confirmed.hash
            && target_event.hash == prepared.target_audit_hash
            && arm.namespace == supervision::namespace(db)?
            && arm.namespace == prepared.namespace
            && arm.session_id == confirmation.session_id.to_string()
            && arm.session_id == parent.launch.session_id
            && parent.admission_operation_id == confirmation.request.operation_id.to_string()
            && admitted.launch.session_id == parent.launch.session_id
            && admitted.admission_operation_id == parent.admission_operation_id
            && admitted.worker_id == parent.worker_id
            && admitted.attempt.attempt_id == parent.attempt.attempt_id
            && admitted.spec_hash == parent.spec_hash
            && parent.state == WorkerStateV1::Reserved
            && parent.revision == admitted.revision
            && parent.quota_held
            && arm.arm.worker_id == parent.worker_id
            && arm.arm.attempt_id == parent.attempt.attempt_id
            && arm.arm.intent_hash == request.intent_hash
            && arm.authorized_at < authority.expires_at
            && fact.authorization_revision <= arm.authorization_revision
            && arm.authorization_revision
                == identity::authorization_revision_before(db, before_seq)?
            && matches!(
                &arm.arm.subject,
                ArmSubjectV1::Helper { purpose: HelperPurposeV1::Import, permission, .. }
                    if matches!(permission.as_ref(), HelperPermissionV1::Admission { receipt } if receipt == &fact.admission)
            ),
    )?;
    conflict(target_broker_at(db, &target.command, arm.authorized_at, before_seq)?.is_some())?;
    workload::validate_broker_identity_at(
        db,
        &fact.actor.historical_identity()?,
        arm.authorized_at,
        before_seq,
    )
}

pub(in crate::audit) fn validate_named_arm_initiator(
    db: &Connection,
    stored: &StoredEvent,
    projection: &Projection,
) -> anyhow::Result<()> {
    let record = admission_codec::decode(stored)?;
    let admission_codec::Input::LegacyNamedArm(arm) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    let fact = arm_admission(db, &arm, stored.seq)?;
    let (_, confirmation, prepared) = admission_origin(db, arm.binding_ref)?;
    let initiator = &prepared.initiator;
    projection.require_local_launch_at(
        &initiator.principal,
        (&initiator.credential_id, initiator.credential_revision),
        (&initiator.membership_id, initiator.membership_revision),
        (&fact.actor.tenant_id, &initiator.organization),
        (stored.seq, arm.authorized_at),
    )?;
    projection.require_resource_at(
        &session_owner_grant(&fact.actor.tenant_id, initiator, confirmation.session_id),
        stored.seq,
    )
}
