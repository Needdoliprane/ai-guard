//! Historical validation of the first Legacy named Import arm.
use super::*;
use crate::{
    audit::{control as control_store, worker_transition},
    control::{
        ArmResultV1, ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1, WorkerMutationV1,
        WorkerStateV1, WorkerTransitionV1, WorkerV1,
    },
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

impl Store {
    /// Commit the named Import Arm or reconcile its exact prior receipt.
    /// The caller supplies the inspected Arm from the broker's retained
    /// checkpoint. Starting the helper has its own authorization boundary.
    #[allow(dead_code, reason = "the private named checkpoint handoff is pending")]
    pub(crate) fn arm_legacy_named_import(
        &self,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
        namespace: &str,
        arm: &ArmV1,
    ) -> anyhow::Result<ArmResultV1> {
        crate::control::invalid(!binding_ref.is_nil())?;
        canonical_uuid(namespace)?;
        arm.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let version = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            format_admits(version) && capability_present(&tx)?,
            ControlFailure::MigrationRequired
        );
        let clock = workload::capture_control_time(&tx)?;

        if let Some((stored, record)) = supervision::load_record(&tx, &arm.operation_id)? {
            let admission_codec::Input::LegacyNamedArm(named) = &record.input else {
                return Err(ControlFailure::Conflict.into());
            };
            let actor = arm_admission(&tx, named, stored.seq)?.actor;
            anyhow::ensure!(actor.matches_binding(broker), ControlFailure::NotFound);
            workload::revalidate_broker_identity_checked(&tx, broker, clock.at())?;
            conflict(
                named.binding_ref == binding_ref
                    && named.namespace == namespace
                    && serde_json::to_value(&named.arm)? == serde_json::to_value(arm)?,
            )?;
            let receipt = supervision::arm_receipt(&named.arm, &named.namespace, &stored)?;
            self.commit_legacy_named_control(tx, &clock, |tx, current| {
                let retained = arm_admission(tx, named, stored.seq)?;
                anyhow::ensure!(
                    retained.actor.matches_binding(broker),
                    ControlFailure::NotFound
                );
                Ok((
                    None,
                    workload::broker_identity_deadline_checked(tx, broker, current.at())?,
                ))
            })?;
            return Ok(ArmResultV1 {
                replayed: true,
                arm: named.arm.clone(),
                receipt,
            });
        }

        let record =
            self.build_legacy_named_import_arm(&tx, broker, binding_ref, namespace, arm, &clock)?;
        let worker = record
            .worker_record
            .as_ref()
            .and_then(|record| record.worker.as_ref())
            .ok_or(ControlFailure::Conflict)?;
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = supervision::arm_receipt(arm, namespace, &stored)?;
        self.commit_legacy_named_control(tx, &clock, |tx, current| {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
            let (seq, hash) = audit_head(tx)?;
            conflict(seq == stored.seq && hash == stored.hash)?;
            let before_seq = seq.checked_add(1).context("audit sequence exhausted")?;
            let actual = control_store::worker_at(tx, &arm.worker_id, before_seq)?
                .ok_or(ControlFailure::NotFound)?;
            conflict(serde_json::to_value(&actual)? == serde_json::to_value(worker)?)?;
            let (human_deadline, broker_deadline) = self.require_legacy_named_execution(
                tx,
                broker,
                binding_ref,
                namespace,
                worker,
                current,
            )?;
            Ok((Some(human_deadline), broker_deadline))
        })?;
        Ok(ArmResultV1 {
            replayed: false,
            arm: arm.clone(),
            receipt,
        })
    }

    /// Build the first named Import record inside the writer's checked
    /// transaction. The caller supplies the existing inspected Arm and
    /// handles exact retained operation replay before entering this path.
    /// The writer must revalidate authority at its final commit boundary.
    fn build_legacy_named_import_arm(
        &self,
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
        namespace: &str,
        arm: &ArmV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<admission_codec::Record> {
        anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
        let before_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let parent = control_store::worker_at(tx, &arm.worker_id, before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        self.require_legacy_named_execution(tx, broker, binding_ref, namespace, &parent, clock)?;
        conflict(supervision::operation_receipt(tx, &arm.operation_id)?.is_none())?;
        let fact = event_by_id(tx, &fact_id(binding_ref))?.ok_or(ControlFailure::NotFound)?;
        let named = LegacyNamedArmV1 {
            version: 1,
            binding_ref,
            admission_fact_seq: fact.seq,
            admission_fact_hash: fact.hash,
            namespace: namespace.into(),
            session_id: parent.launch.session_id.clone(),
            authorization_revision: identity::authorization_revision(tx)?,
            authorized_at: clock.at(),
            arm: arm.clone(),
        };
        legacy_named_arm_predecessor(tx, &named, &parent, before_seq)?;
        let prepare = WorkerMutationV1 {
            version: 1,
            operation_id: arm.operation_id.clone(),
            worker_id: parent.worker_id.clone(),
            attempt_id: parent.attempt.attempt_id.clone(),
            expected_revision: parent.revision,
            transition: WorkerTransitionV1::Prepare,
        };
        let mutation_hash = worker_transition::validate_input(&prepare, &self.redactor)?;
        let prepared = worker_transition::build_record(parent, &prepare, mutation_hash)?;
        arm.validate_for_parent(prepared.worker.as_ref().ok_or(ControlFailure::Conflict)?)?;
        let input = admission_codec::Input::LegacyNamedArm(Box::new(named));
        admission_codec::validate_input(&input, &self.redactor)?;
        Ok(admission_codec::Record::new(input, Some(prepared)))
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
