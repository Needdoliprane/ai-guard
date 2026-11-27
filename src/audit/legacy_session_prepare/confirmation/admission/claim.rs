//! One checked transaction for the first named Legacy Import claim.
use super::*;
use crate::{
    audit::control as control_store,
    control::{
        ArmV1, ClaimDispositionV1, HelperClaimResultV1, WorkerV1, invalid,
        session_control::LegacyNamedImportClaimRequestV1,
    },
};

pub(in crate::audit) const LEGACY_NAMED_IMPORT_CLAIM_KIND: &str =
    "control.execution.helper_claim.legacy_named";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportClaimV1 {
    pub(in crate::audit) version: u32,
    pub(in crate::audit) request: LegacyNamedImportClaimRequestV1,
    pub(in crate::audit) session_id: String,
    pub(in crate::audit) authorization_revision: u64,
    pub(in crate::audit) authorized_at: DateTime<Utc>,
}

impl LegacyNamedImportClaimV1 {
    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        invalid(
            self.version == 1
                && self.authorization_revision > 0
                && workload::canonical_control_time(self.authorized_at)? == self.authorized_at,
        )?;
        canonical_uuid(&self.session_id)?;
        // The event hash also covers the Store's observed authorization.
        self.request.input_hash()
    }
}

fn claim_admission(
    db: &Connection,
    binding_ref: Uuid,
) -> anyhow::Result<(StoredEvent, AdmissionFact)> {
    let stored = event_by_id(db, &fact_id(binding_ref))?.ok_or(ControlFailure::NotFound)?;
    let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
    let grant = fact.grant.as_ref().ok_or(ControlFailure::NotFound)?;
    conflict(
        fact.version == 1
            && fact.binding_ref == binding_ref
            && fact.admission.audit_seq < stored.seq
            && grant.audit_seq < stored.seq,
    )?;
    Ok((stored, fact))
}

fn claim_arm_at(
    db: &Connection,
    request: &LegacyNamedImportClaimRequestV1,
    admission: &StoredEvent,
    before_seq: i64,
) -> anyhow::Result<LegacyNamedArmV1> {
    let receipt = &request.claim.arm_receipt;
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events \
             WHERE seq=?1 AND seq<?2 AND kind=?3",
            params![receipt.audit_seq, before_seq, LEGACY_NAMED_ARM_KIND],
            row_event,
        )
        .optional()?
        .ok_or(ControlFailure::NotFound)?;
    let record = admission_codec::decode(&stored)?;
    let admission_codec::Input::LegacyNamedArm(arm) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        arm.binding_ref == request.binding_ref
            && arm.namespace == request.namespace
            && arm.arm.worker_id == receipt.worker_id
            && arm.arm.attempt_id == receipt.attempt_id,
        ControlFailure::NotFound
    );
    conflict(
        arm.admission_fact_seq == admission.seq
            && arm.admission_fact_hash == admission.hash
            && admission.seq < stored.seq,
    )?;
    let actual = supervision::arm_receipt(&arm.arm, &arm.namespace, &stored)?;
    request.validate_for_arm(arm.binding_ref, &arm.namespace, &arm.arm, &actual)?;
    Ok(*arm)
}

pub(in crate::audit) fn legacy_named_import_claim_predecessor(
    db: &Connection,
    stored: &StoredEvent,
    claim: &LegacyNamedImportClaimV1,
    parent: &WorkerV1,
) -> anyhow::Result<()> {
    let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    conflict(format_admits(version))?;
    let (admission, fact) = claim_admission(db, claim.request.binding_ref)?;
    let arm = claim_arm_at(db, &claim.request, &admission, stored.seq)?;
    let (confirmed, confirmation, prepared) = admission_origin(db, claim.request.binding_ref)?;
    let authority = confirmation
        .admission_authority
        .as_ref()
        .filter(|_| confirmation.version == 2)
        .ok_or(ControlFailure::NotFound)?;
    let (target_event, target) = require_target_at(
        db,
        confirmation.request.selection()?,
        &prepared.initiator,
        stored.seq,
    )?;
    conflict(
        stored.event.session_id.as_deref() == Some(claim.session_id.as_str())
            && stored.event.coverage == "observed"
            && claim.session_id == arm.session_id
            && claim.session_id == parent.launch.session_id
            && parent.admission_operation_id == fact.admission.operation_id
            && arm.namespace == supervision::namespace(db)?
            && confirmed.hash == fact.confirmation_hash
            && target_event.hash == prepared.target_audit_hash
            && claim.authorized_at >= arm.authorized_at
            && claim.authorized_at <= crate::broker_registry::parse_time(&stored.received_at)?
            && claim.authorized_at < authority.expires_at
            && claim.authorization_revision >= arm.authorization_revision
            && claim.authorization_revision
                == identity::authorization_revision_before(db, stored.seq)?,
    )?;
    conflict(target_broker_at(db, &target.command, claim.authorized_at, stored.seq)?.is_some())?;
    workload::validate_broker_identity_at(
        db,
        &fact.actor.historical_identity()?,
        claim.authorized_at,
        stored.seq,
    )
}

pub(in crate::audit) fn validate_named_import_claim_initiator(
    db: &Connection,
    stored: &StoredEvent,
    projection: &Projection,
) -> anyhow::Result<()> {
    let record = admission_codec::decode(stored)?;
    let admission_codec::Input::LegacyNamedImportClaim(claim) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    let (_, fact) = claim_admission(db, claim.request.binding_ref)?;
    let (_, confirmation, prepared) = admission_origin(db, claim.request.binding_ref)?;
    let initiator = &prepared.initiator;
    projection.require_local_launch_at(
        &initiator.principal,
        (&initiator.credential_id, initiator.credential_revision),
        (&initiator.membership_id, initiator.membership_revision),
        (&fact.actor.tenant_id, &initiator.organization),
        (stored.seq, claim.authorized_at),
    )?;
    projection.require_resource_at(
        &session_owner_grant(&fact.actor.tenant_id, initiator, confirmation.session_id),
        stored.seq,
    )
}

struct AuthorizedClaimLookup {
    clock: workload::AuthorizedControlTimeV1,
    broker_deadline: DateTime<Utc>,
    arm: ArmV1,
    before_seq: i64,
}

impl Store {
    fn authorize_named_import_claim_lookup(
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportClaimRequestV1,
    ) -> anyhow::Result<AuthorizedClaimLookup> {
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
        let arm = claim_arm_at(tx, request, &admission, before_seq)?.arm;
        Ok(AuthorizedClaimLookup {
            clock,
            broker_deadline,
            arm,
            before_seq,
        })
    }

    fn finish_named_import_claim_lookup(
        &self,
        tx: CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        lookup: &AuthorizedClaimLookup,
    ) -> anyhow::Result<()> {
        self.commit_legacy_named_control(tx, &lookup.clock, |tx, current| {
            Ok((
                None,
                workload::broker_identity_deadline_checked(tx, broker, current.at())?,
            ))
        })?;
        require_claim_release_deadlines(None, lookup.broker_deadline)
    }

    /// An exact consumed claim remains readable after the human authority ends.
    /// Only an authorized first commit can return the initial disposition.
    #[allow(
        dead_code,
        reason = "the private broker claim route is composed separately"
    )]
    pub(crate) fn claim_legacy_named_import(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportClaimRequestV1,
    ) -> anyhow::Result<HelperClaimResultV1> {
        let input_hash = request.input_hash()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = Self::authorize_named_import_claim_lookup(&tx, broker, request)?;
        if let Some(retained) = retained_named_import_claim(&tx, request, &input_hash)? {
            self.finish_named_import_claim_lookup(tx, broker, &lookup)?;
            return Ok(retained);
        }

        anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
        conflict(!named_operation(
            &tx,
            canonical_uuid(&request.claim.operation_id)?,
        )?)?;
        let arm = &lookup.arm;
        let parent = control_store::worker_at(&tx, &arm.worker_id, lookup.before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        arm.validate_for_parent(&parent)?;
        let (human_deadline, broker_deadline) = self.require_legacy_named_execution(
            &tx,
            broker,
            request.binding_ref,
            &request.namespace,
            &parent,
            &lookup.clock,
        )?;
        let input =
            admission_codec::Input::LegacyNamedImportClaim(Box::new(LegacyNamedImportClaimV1 {
                version: 1,
                request: request.clone(),
                session_id: parent.launch.session_id.clone(),
                authorization_revision: identity::authorization_revision(&tx)?,
                authorized_at: lookup.clock.at(),
            }));
        admission_codec::validate_input(&input, &self.redactor)?;
        let record = admission_codec::Record::new(input, None);
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = admission_codec::receipt_for(&stored, &record)?;
        self.commit_legacy_named_control(tx, &lookup.clock, |tx, current| {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
            let (seq, hash) = audit_head(tx)?;
            conflict(seq == stored.seq && hash == stored.hash)?;
            let (human, broker) = self.require_legacy_named_execution(
                tx,
                broker,
                request.binding_ref,
                &request.namespace,
                &parent,
                current,
            )?;
            Ok((Some(human), broker))
        })?;
        require_claim_release_deadlines(Some(human_deadline), broker_deadline)?;
        Ok(HelperClaimResultV1 {
            receipt,
            disposition: ClaimDispositionV1::EmitOnce,
        })
    }

    /// Reconcile the exact consumed claim using only the bound broker's current
    /// identity. An absent receipt only describes this transaction's snapshot.
    #[allow(
        dead_code,
        reason = "the private broker reconciliation route is composed separately"
    )]
    pub(crate) fn read_legacy_named_import_claim(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportClaimRequestV1,
    ) -> anyhow::Result<Option<HelperClaimResultV1>> {
        let input_hash = request.input_hash()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let lookup = Self::authorize_named_import_claim_lookup(&tx, broker, request)?;
        let retained = retained_named_import_claim(&tx, request, &input_hash)?;
        self.finish_named_import_claim_lookup(tx, broker, &lookup)?;
        Ok(retained)
    }
}

fn retained_named_import_claim(
    db: &Connection,
    request: &LegacyNamedImportClaimRequestV1,
    input_hash: &str,
) -> anyhow::Result<Option<HelperClaimResultV1>> {
    let Some((stored, record)) = supervision::load_record(db, &request.claim.operation_id)? else {
        return Ok(None);
    };
    let admission_codec::Input::LegacyNamedImportClaim(retained) = &record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    anyhow::ensure!(
        retained.request.binding_ref == request.binding_ref
            && retained.request.namespace == request.namespace,
        ControlFailure::NotFound
    );
    conflict(retained.input_hash()? == input_hash)?;
    Ok(Some(HelperClaimResultV1 {
        receipt: admission_codec::receipt_for(&stored, &record)?,
        disposition: ClaimDispositionV1::AlreadyConsumed,
    }))
}

fn require_claim_release_deadlines(
    human_deadline: Option<DateTime<Utc>>,
    broker_deadline: DateTime<Utc>,
) -> anyhow::Result<()> {
    // A commit that outlasts the authorization window remains consumed.
    let now = Utc::now();
    anyhow::ensure!(
        human_deadline.is_none_or(|deadline| now < deadline),
        ControlFailure::NotFound
    );
    anyhow::ensure!(
        workload::canonical_control_time(now)? < broker_deadline,
        WorkloadFailureV1::Unauthorized
    );
    Ok(())
}
