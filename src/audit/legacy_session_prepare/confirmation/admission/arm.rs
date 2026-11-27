//! Historical validation of the first Legacy named Import arm.
use super::*;
use crate::{
    audit::{control as control_store, worker_transition},
    control::{
        ArmResultV1, ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1, WorkerMutationV1,
        WorkerStateV1, WorkerTransitionV1, WorkerV1,
        session_control::{
            LegacyNamedImportArmRequestV1, LegacyNamedImportVerificationArmRequestV2,
        },
    },
};

#[path = "verification_arm.rs"]
mod verification;

pub(in crate::audit) const LEGACY_NAMED_ARM_KIND: &str = "control.execution.arm.legacy_named";
pub(in crate::audit) const LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND: &str =
    "control.execution.arm.legacy_named_import_verification";

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
    fn validate_authorization(&self) -> anyhow::Result<()> {
        crate::control::invalid(
            self.version == 1
                && !self.binding_ref.is_nil()
                && self.admission_fact_seq > 0
                && workload::canonical_control_time(self.authorized_at)? == self.authorized_at,
        )?;
        canonical_uuid(&self.namespace)?;
        canonical_uuid(&self.session_id)?;
        crate::control::hash(&self.admission_fact_hash)
    }

    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        self.validate_authorization()?;
        crate::control::invalid(matches!(
            &self.arm.subject,
            ArmSubjectV1::Helper {
                purpose: HelperPurposeV1::Import,
                ..
            }
        ))?;
        self.arm.validate()?;
        crate::control::content_hash("ai-guard:legacy-named-import-arm:v1", self)
    }

    pub(in crate::audit) fn verification_input_hash(&self) -> anyhow::Result<String> {
        self.validate_authorization()?;
        LegacyNamedImportVerificationArmRequestV2::validate_arm(
            self.binding_ref,
            &self.namespace,
            &self.arm,
        )?;
        crate::control::content_hash("ai-guard:legacy-named-import-verification-arm:v2", self)
    }
}

impl Store {
    /// Commit the named Import Arm or reconcile its exact prior receipt.
    /// The caller supplies the inspected Arm from the broker's retained
    /// checkpoint. Starting the helper has its own authorization boundary.
    pub(crate) fn arm_legacy_named_import(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportArmRequestV1,
    ) -> anyhow::Result<ArmResultV1> {
        request.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let (clock, admission) = authorize_named_import_lookup(&tx, broker, request)?;
        if let Some(retained) = retained_named_import_arm(&tx, request)? {
            self.finish_named_import_lookup(tx, broker, &clock)?;
            return Ok(retained);
        }

        let arm = &request.arm;
        // A retained Arm may predate the derived operation identity. Require
        // it only when writing the first fact and protect the control ledger.
        let operation = LegacyNamedImportArmRequestV1::operation_id(
            request.binding_ref,
            canonical_uuid(&arm.attempt_id)?,
        );
        conflict(
            arm.operation_id == operation && !named_operation(&tx, canonical_uuid(&operation)?)?,
        )?;
        let record =
            self.build_legacy_named_import_arm(&tx, broker, request, &clock, &admission)?;
        let worker = record
            .worker_record
            .as_ref()
            .and_then(|record| record.worker.as_ref())
            .ok_or(ControlFailure::Conflict)?;
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = supervision::arm_receipt(arm, &request.namespace, &stored)?;
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
                request.binding_ref,
                &request.namespace,
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

    /// Look up the same exact Arm for the currently authenticated broker. An
    /// absent record is only an observation of this transaction.
    pub(crate) fn read_legacy_named_import_arm(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportArmRequestV1,
    ) -> anyhow::Result<Option<ArmResultV1>> {
        request.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let (clock, _) = authorize_named_import_lookup(&tx, broker, request)?;
        let retained = retained_named_import_arm(&tx, request)?;
        self.finish_named_import_lookup(tx, broker, &clock)?;
        Ok(retained)
    }

    fn finish_named_import_lookup(
        &self,
        tx: CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<()> {
        self.commit_legacy_named_control(tx, clock, |tx, current| {
            Ok((
                None,
                workload::broker_identity_deadline_checked(tx, broker, current.at())?,
            ))
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
        request: &LegacyNamedImportArmRequestV1,
        clock: &workload::AuthorizedControlTimeV1,
        admission: &StoredEvent,
    ) -> anyhow::Result<admission_codec::Record> {
        anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
        let arm = &request.arm;
        let before_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let parent = control_store::worker_at(tx, &arm.worker_id, before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        self.require_legacy_named_execution(
            tx,
            broker,
            request.binding_ref,
            &request.namespace,
            &parent,
            clock,
        )?;
        conflict(supervision::operation_receipt(tx, &arm.operation_id)?.is_none())?;
        let named = LegacyNamedArmV1 {
            version: 1,
            binding_ref: request.binding_ref,
            admission_fact_seq: admission.seq,
            admission_fact_hash: admission.hash.clone(),
            namespace: request.namespace.clone(),
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

fn authorize_named_import_lookup(
    tx: &CheckedTransaction<'_>,
    broker: &BrokerIdentityV1,
    request: &LegacyNamedImportArmRequestV1,
) -> anyhow::Result<(workload::AuthorizedControlTimeV1, StoredEvent)> {
    let lookup = authorize_named_arm_subject(
        tx,
        broker,
        request.binding_ref,
        &request.namespace,
        &request.arm,
    )?;
    conflict(
        lookup.admitted_revision.checked_add(1) == Some(request.arm.expected_revision)
            && matches!(
                &request.arm.subject,
                ArmSubjectV1::Helper { permission, .. }
                    if matches!(permission.as_ref(), HelperPermissionV1::Admission { receipt } if receipt == &lookup.receipt)
            ),
    )?;
    Ok((lookup.clock, lookup.admission))
}

struct AuthorizedNamedArmSubject {
    clock: workload::AuthorizedControlTimeV1,
    admission: StoredEvent,
    admitted_revision: u64,
    receipt: MutationReceiptV1,
}

fn authorize_named_arm_subject(
    tx: &CheckedTransaction<'_>,
    broker: &BrokerIdentityV1,
    binding_ref: Uuid,
    namespace: &str,
    arm: &ArmV1,
) -> anyhow::Result<AuthorizedNamedArmSubject> {
    Store::require_legacy_named_admission_format(tx)?;
    let clock = workload::capture_control_time(tx)?;
    workload::revalidate_broker_identity_checked(tx, broker, clock.at())?;
    let stored = event_by_id(tx, &fact_id(binding_ref))?.ok_or(ControlFailure::NotFound)?;
    let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
    anyhow::ensure!(
        fact.actor.matches_binding(broker)
            && fact.grant.is_some()
            && namespace == supervision::namespace(tx)?,
        ControlFailure::NotFound
    );
    conflict(
        fact.version == 1
            && fact.binding_ref == binding_ref
            && fact.admission.audit_seq < stored.seq,
    )?;
    let (_, record) = admission_record(tx, &fact)?;
    let (admission, worker_record) = record.admission()?;
    let worker = worker_record
        .worker
        .as_ref()
        .ok_or(ControlFailure::NotFound)?;
    anyhow::ensure!(
        arm.worker_id == worker.worker_id && arm.attempt_id == worker.attempt.attempt_id,
        ControlFailure::NotFound
    );
    conflict(arm.intent_hash == admission.intent_hash)?;
    Ok(AuthorizedNamedArmSubject {
        clock,
        admission: stored,
        admitted_revision: worker.revision,
        receipt: fact.admission,
    })
}

fn retained_named_import_arm(
    db: &Connection,
    request: &LegacyNamedImportArmRequestV1,
) -> anyhow::Result<Option<ArmResultV1>> {
    let Some((stored, record)) = supervision::load_record(db, &request.arm.operation_id)? else {
        return Ok(None);
    };
    let admission_codec::Input::LegacyNamedArm(named) = &record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        named.binding_ref == request.binding_ref && named.namespace == request.namespace,
        ControlFailure::NotFound
    );
    arm_admission(db, named, stored.seq)?;
    conflict(serde_json::to_value(&named.arm)? == serde_json::to_value(&request.arm)?)?;
    Ok(Some(ArmResultV1 {
        replayed: true,
        arm: named.arm.clone(),
        receipt: supervision::arm_receipt(&named.arm, &named.namespace, &stored)?,
    }))
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
    validate_named_arm_authority(db, arm, parent, before_seq, |fact, admitted_revision| {
        conflict(
            parent.state == WorkerStateV1::Reserved
                && parent.revision == admitted_revision
                && matches!(
                    &arm.arm.subject,
                    ArmSubjectV1::Helper { purpose: HelperPurposeV1::Import, permission, .. }
                        if matches!(permission.as_ref(), HelperPermissionV1::Admission { receipt } if receipt == &fact.admission)
                ),
        )
    })
}

fn validate_named_arm_authority(
    db: &Connection,
    arm: &LegacyNamedArmV1,
    parent: &WorkerV1,
    before_seq: i64,
    validate_phase: impl FnOnce(&AdmissionFact, u64) -> anyhow::Result<()>,
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
            && parent.quota_held
            && arm.arm.worker_id == parent.worker_id
            && arm.arm.attempt_id == parent.attempt.attempt_id
            && arm.arm.intent_hash == request.intent_hash
            && arm.authorized_at < authority.expires_at
            && fact.authorization_revision <= arm.authorization_revision
            && arm.authorization_revision
                == identity::authorization_revision_before(db, before_seq)?,
    )?;
    validate_phase(&fact, admitted.revision)?;
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
    let (admission_codec::Input::LegacyNamedArm(arm)
    | admission_codec::Input::LegacyNamedImportVerificationArm(arm)) = record.input
    else {
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
