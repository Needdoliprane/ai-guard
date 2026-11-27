//! Close a named Legacy reservation before any arm or claim was admitted.
use super::*;
use crate::{
    audit::{control as control_store, worker_admission::ControlRecord, worker_transition},
    control::{MutationResultV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedCancelReservationV1 {
    pub(crate) version: u32,
    pub(crate) binding_ref: Uuid,
    pub(crate) namespace: String,
    pub(crate) admission: MutationReceiptV1,
    pub(crate) mutation: WorkerMutationV1,
}

impl LegacyNamedCancelReservationV1 {
    pub(crate) fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::invalid(
            self.version == 1
                && !self.binding_ref.is_nil()
                && matches!(
                    &self.mutation.transition,
                    WorkerTransitionV1::CancelReserved
                ),
        )?;
        canonical_uuid(&self.namespace)?;
        worker_transition::validate_identity(&self.mutation)?;
        crate::control::content_hash("ai-guard:legacy-named-cancel-reservation:v1", self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedCancelV1 {
    pub(in crate::audit) request: LegacyNamedCancelReservationV1,
    authorized_at: DateTime<Utc>,
}

impl LegacyNamedCancelV1 {
    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::invalid(
            workload::canonical_control_time(self.authorized_at)? == self.authorized_at,
        )?;
        // The audit event retains the authority observation. The operation hash
        // remains stable across receipt reconciliation.
        self.request.input_hash()
    }
}

fn cancellation_admission(
    db: &Connection,
    binding_ref: Uuid,
    before_seq: i64,
) -> anyhow::Result<AdmissionFact> {
    let stored = event_by_id(db, &fact_id(binding_ref))?.ok_or(ControlFailure::NotFound)?;
    let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
    conflict(
        fact.version == 1
            && fact.binding_ref == binding_ref
            && fact.admission.audit_seq < stored.seq
            && fact.grant.is_some()
            && stored.seq < before_seq,
    )?;
    Ok(fact)
}

fn require_cancel_predecessor(
    db: &Connection,
    fact: &AdmissionFact,
    request: &LegacyNamedCancelReservationV1,
    parent: &WorkerV1,
) -> anyhow::Result<()> {
    let mutation = &request.mutation;
    conflict(
        request.namespace == supervision::namespace(db)?
            && request.admission == fact.admission
            && mutation.worker_id == parent.worker_id
            && mutation.attempt_id == parent.attempt.attempt_id
            && mutation.expected_revision == parent.revision
            && parent.state == WorkerStateV1::Reserved
            && parent.quota_held,
    )?;
    let (_, record) = admission_record(db, fact)?;
    let (_, worker_record) = record.admission()?;
    let admitted = worker_record
        .worker
        .as_ref()
        .ok_or(ControlFailure::NotFound)?;
    conflict(serde_json::to_value(admitted)? == serde_json::to_value(parent)?)
}

pub(in crate::audit) fn validate_legacy_named_cancel(
    db: &Connection,
    stored: &StoredEvent,
    cancel: &LegacyNamedCancelV1,
    parent: &WorkerV1,
    transition: &ControlRecord,
) -> anyhow::Result<()> {
    let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    conflict(
        format_admits(version)
            && stored.event.session_id.as_deref() == Some(parent.launch.session_id.as_str())
            && stored.event.coverage == "observed"
            && cancel.authorized_at <= crate::broker_registry::parse_time(&stored.received_at)?
            && transition.action == "cancel_reserved"
            && transition.operation_id == cancel.request.mutation.operation_id,
    )?;
    let fact = cancellation_admission(db, cancel.request.binding_ref, stored.seq)?;
    require_cancel_predecessor(db, &fact, &cancel.request, parent)?;
    workload::validate_broker_identity_at(
        db,
        &fact.actor.historical_identity()?,
        cancel.authorized_at,
        stored.seq,
    )?;
    worker_transition::replay_record(parent, transition)
}

fn cancellation_result(
    stored: &StoredEvent,
    record: &admission_codec::Record,
) -> anyhow::Result<MutationResultV1> {
    let worker = record
        .worker_record
        .as_ref()
        .and_then(|record| record.worker.clone())
        .ok_or(ControlFailure::Conflict)?;
    Ok(MutationResultV1 {
        worker,
        receipt: admission_codec::receipt_for(stored, record)?,
        disposition: None,
    })
}

impl Store {
    /// Release the original untouched reservation on explicit broker abandon,
    /// including after human authority was withdrawn or supervision reopened.
    /// The Store establishes the absence of a durable arm or claim. The broker
    /// must independently establish that no runtime work preceded this boundary.
    pub(crate) fn cancel_legacy_named_reservation(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedCancelReservationV1,
    ) -> anyhow::Result<MutationResultV1> {
        let input_hash = request.input_hash()?;
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
        workload::revalidate_broker_identity_checked(&tx, broker, clock.at())?;
        let before_seq = audit_head(&tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let fact = cancellation_admission(&tx, request.binding_ref, before_seq)?;
        anyhow::ensure!(fact.actor.matches_binding(broker), ControlFailure::NotFound);

        let result = if let Some((stored, record)) =
            supervision::load_record(&tx, &request.mutation.operation_id)?
        {
            let admission_codec::Input::LegacyNamedCancel(cancel) = &record.input else {
                return Err(ControlFailure::Conflict.into());
            };
            let fact = cancellation_admission(&tx, cancel.request.binding_ref, stored.seq)?;
            anyhow::ensure!(fact.actor.matches_binding(broker), ControlFailure::NotFound);
            conflict(cancel.request.input_hash()? == input_hash)?;
            cancellation_result(&stored, &record)?
        } else {
            let parent = control_store::worker_at(&tx, &request.mutation.worker_id, before_seq)?
                .ok_or(ControlFailure::NotFound)?;
            require_cancel_predecessor(&tx, &fact, request, &parent)?;
            let mutation = &request.mutation;
            let hash = worker_transition::validate_input(mutation, &self.redactor)?;
            let transition = worker_transition::build_record(parent, mutation, hash)?;
            let input = admission_codec::Input::LegacyNamedCancel(Box::new(LegacyNamedCancelV1 {
                request: request.clone(),
                authorized_at: clock.at(),
            }));
            admission_codec::validate_input(&input, &self.redactor)?;
            let record = admission_codec::Record::new(input, Some(transition));
            let stored = self.persist_supervision_record(&tx, &record)?;
            self.validate_supervision_write(&tx)?;
            cancellation_result(&stored, &record)?
        };

        self.commit_legacy_named_control(tx, &clock, |tx, current| {
            Ok((
                None,
                workload::broker_identity_deadline_checked(tx, broker, current.at())?,
            ))
        })?;
        Ok(result)
    }
}
