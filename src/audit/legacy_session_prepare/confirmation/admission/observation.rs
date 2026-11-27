//! Audit and reconcile observations of the exact consumed named Import claim.
use super::*;
use crate::{
    audit::control::supervision::LegacyNamedImportObservationV1,
    control::session_control::{
        LegacyNamedImportObservationPhaseV1, LegacyNamedImportObservationRequestV1,
    },
};

fn observation_claim(
    db: &Connection,
    request: &LegacyNamedImportObservationRequestV1,
    admission: &StoredEvent,
    before_seq: i64,
) -> anyhow::Result<LegacyNamedImportClaimV1> {
    let (stored, record) = supervision::load_record(db, &request.claim_receipt.operation_id)?
        .ok_or(ControlFailure::NotFound)?;
    let admission_codec::Input::LegacyNamedImportClaim(claim) = &record.input else {
        return Err(ControlFailure::NotFound.into());
    };
    anyhow::ensure!(
        claim.request.binding_ref == request.binding_ref
            && claim.request.namespace == request.namespace
            && stored.seq < before_seq,
        ControlFailure::NotFound
    );
    let arm = claim_arm_at(db, &claim.request, admission, stored.seq)?;
    conflict(
        admission_codec::receipt_for(&stored, &record)? == request.claim_receipt
            && serde_json::to_value(&claim.request.claim.arm_receipt)?
                == serde_json::to_value(&request.observation.arm_receipt)?
            && claim.request.claim.helper_id == request.observation.helper_id
            && arm.arm.inspection == request.observation.observation.inspection
            && arm.session_id == claim.session_id,
    )?;
    Ok(claim.as_ref().clone())
}

fn require_preceding_started(
    db: &Connection,
    request: &LegacyNamedImportObservationRequestV1,
    before_seq: i64,
) -> anyhow::Result<()> {
    if request.phase != LegacyNamedImportObservationPhaseV1::Terminal {
        return Ok(());
    }
    let operation_id = LegacyNamedImportObservationRequestV1::operation_id(
        request.binding_ref,
        canonical_uuid(&request.claim_receipt.operation_id)?,
        LegacyNamedImportObservationPhaseV1::Started,
    );
    let Some((_, record)) =
        supervision::load_record(db, &operation_id)?.filter(|(stored, _)| stored.seq < before_seq)
    else {
        return Ok(());
    };
    let admission_codec::Input::LegacyNamedImportObservation(previous) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    conflict(
        previous.request.phase == LegacyNamedImportObservationPhaseV1::Started
            && previous.request.claim_receipt == request.claim_receipt
            && previous.request.observation.observation.started_at
                == request.observation.observation.started_at,
    )
}

struct AuthorizedObservationLookup {
    clock: workload::AuthorizedControlTimeV1,
    broker_deadline: DateTime<Utc>,
    claim: LegacyNamedImportClaimV1,
    before_seq: i64,
}

impl Store {
    pub(in crate::audit) fn validate_named_import_observation_predecessor(
        db: &Connection,
        stored: &StoredEvent,
        observation: &LegacyNamedImportObservationV1,
    ) -> anyhow::Result<()> {
        let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        conflict(format_admits(version))?;
        let request = &observation.request;
        let (admission, fact) = claim_admission(db, request.binding_ref)?;
        let claim = observation_claim(db, request, &admission, stored.seq)?;
        conflict(
            stored.event.session_id.as_deref() == Some(observation.session_id.as_str())
                && stored.event.coverage == "observed"
                && observation.session_id == claim.session_id
                && request.namespace == supervision::namespace(db)?
                && observation.authorized_at >= claim.authorized_at
                && observation.authorized_at
                    <= crate::broker_registry::parse_time(&stored.received_at)?,
        )?;
        require_preceding_started(db, request, stored.seq)?;
        workload::validate_broker_identity_at(
            db,
            &fact.actor.historical_identity()?,
            observation.authorized_at,
            stored.seq,
        )
    }

    fn authorize_named_import_observation_lookup(
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportObservationRequestV1,
    ) -> anyhow::Result<AuthorizedObservationLookup> {
        Self::require_legacy_named_admission_format(tx)?;
        let clock = workload::capture_control_time(tx)?;
        let broker_deadline = workload::broker_identity_deadline_checked(tx, broker, clock.at())?;
        let (admission, fact) = claim_admission(tx, request.binding_ref)?;
        anyhow::ensure!(
            fact.actor.matches_binding(broker) && request.namespace == supervision::namespace(tx)?,
            ControlFailure::NotFound
        );
        let before_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let claim = observation_claim(tx, request, &admission, before_seq)?;
        Ok(AuthorizedObservationLookup {
            clock,
            broker_deadline,
            claim,
            before_seq,
        })
    }

    fn finish_named_import_observation<T>(
        &self,
        tx: CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        lookup: &AuthorizedObservationLookup,
        written: Option<&StoredEvent>,
        publish: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let response = publish()?;
        self.commit_legacy_named_control(tx, &lookup.clock, |tx, current| {
            if let Some(written) = written {
                let (seq, hash) = audit_head(tx)?;
                conflict(seq == written.seq && hash == written.hash)?;
            }
            Ok((
                None,
                workload::broker_identity_deadline_checked(tx, broker, current.at())?,
            ))
        })?;
        require_claim_release_deadlines(None, lookup.broker_deadline)?;
        Ok(response)
    }

    /// Persist an observed fact under the bound broker's current authority.
    /// The consumed claim is historical and conveys no new execution authority.
    pub(crate) fn observe_legacy_named_import<T>(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportObservationRequestV1,
        publish: impl FnOnce(&MutationReceiptV1, bool) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let input_hash = request.input_hash()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = Self::authorize_named_import_observation_lookup(&tx, broker, request)?;
        if let Some(retained) = retained_observation(&tx, request, &input_hash)? {
            return self.finish_named_import_observation(tx, broker, &lookup, None, || {
                publish(&retained, true)
            });
        }

        conflict(!named_operation(
            &tx,
            canonical_uuid(&request.observation.operation_id)?,
        )?)?;
        require_preceding_started(&tx, request, lookup.before_seq)?;
        let input = admission_codec::Input::LegacyNamedImportObservation(Box::new(
            LegacyNamedImportObservationV1 {
                version: 1,
                request: request.clone(),
                session_id: lookup.claim.session_id.clone(),
                authorized_at: lookup.clock.at(),
            },
        ));
        admission_codec::validate_input(&input, &self.redactor)?;
        let record = admission_codec::Record::new(input, None);
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = admission_codec::receipt_for(&stored, &record)?;
        request.validate_receipt(&receipt)?;
        self.finish_named_import_observation(tx, broker, &lookup, Some(&stored), || {
            publish(&receipt, false)
        })
    }

    /// Reconcile an exact fact after the human authorization ends. An
    /// authoritative absence is available only for the bound broker and claim.
    pub(crate) fn read_legacy_named_import_observation<T>(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportObservationRequestV1,
        publish: impl FnOnce(Option<&MutationReceiptV1>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let input_hash = request.input_hash()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = Self::authorize_named_import_observation_lookup(&tx, broker, request)?;
        let retained = retained_observation(&tx, request, &input_hash)?;
        self.finish_named_import_observation(tx, broker, &lookup, None, || {
            publish(retained.as_ref())
        })
    }
}

fn retained_observation(
    db: &Connection,
    request: &LegacyNamedImportObservationRequestV1,
    input_hash: &str,
) -> anyhow::Result<Option<MutationReceiptV1>> {
    let Some((stored, record)) = supervision::load_record(db, &request.observation.operation_id)?
    else {
        return Ok(None);
    };
    let admission_codec::Input::LegacyNamedImportObservation(retained) = &record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        retained.request.binding_ref == request.binding_ref
            && retained.request.namespace == request.namespace,
        ControlFailure::NotFound
    );
    conflict(record.input.hash()? == input_hash)?;
    let receipt = admission_codec::receipt_for(&stored, &record)?;
    request.validate_receipt(&receipt)?;
    Ok(Some(receipt))
}
