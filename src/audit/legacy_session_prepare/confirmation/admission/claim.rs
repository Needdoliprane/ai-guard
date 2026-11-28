//! Checked claims for the two named Legacy Import helpers.
use super::*;
use crate::{
    audit::control as control_store,
    control::{
        ArmV1, ClaimDispositionV1, HelperClaimResultV1, HelperStartClaimV1, WorkerV1, invalid,
        session_control::{
            LegacyNamedImportClaimRequestV1, LegacyNamedImportVerificationClaimRequestV2,
        },
    },
};

#[path = "observation.rs"]
mod observation;

pub(in crate::audit) const LEGACY_NAMED_IMPORT_CLAIM_KIND: &str =
    "control.execution.helper_claim.legacy_named";
pub(in crate::audit) const LEGACY_NAMED_IMPORT_VERIFICATION_CLAIM_KIND: &str =
    "control.execution.helper_claim.legacy_named_import_verification";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedHelperClaim<R> {
    pub(in crate::audit) version: u32,
    pub(in crate::audit) request: R,
    pub(in crate::audit) session_id: String,
    pub(in crate::audit) authorization_revision: u64,
    pub(in crate::audit) authorized_at: DateTime<Utc>,
}

pub(crate) type LegacyNamedImportClaimV1 = LegacyNamedHelperClaim<LegacyNamedImportClaimRequestV1>;
pub(crate) type LegacyNamedImportVerificationClaimV2 =
    LegacyNamedHelperClaim<LegacyNamedImportVerificationClaimRequestV2>;

impl<R> LegacyNamedHelperClaim<R> {
    fn validate_authorization(&self, version: u32) -> anyhow::Result<()> {
        invalid(
            self.version == version
                && self.authorization_revision > 0
                && workload::canonical_control_time(self.authorized_at)? == self.authorized_at,
        )?;
        canonical_uuid(&self.session_id)?;
        Ok(())
    }
}

impl LegacyNamedImportClaimV1 {
    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        self.validate_authorization(1)?;
        // The event hash also covers the Store's observed authorization.
        self.request.input_hash()
    }
}

impl LegacyNamedImportVerificationClaimV2 {
    pub(in crate::audit) fn input_hash(&self) -> anyhow::Result<String> {
        self.validate_authorization(2)?;
        self.request.input_hash()
    }
}

#[derive(Clone, Copy)]
enum ClaimRequest<'a> {
    Import(&'a LegacyNamedImportClaimRequestV1),
    Verification(&'a LegacyNamedImportVerificationClaimRequestV2),
}

impl<'a> ClaimRequest<'a> {
    fn parts(self) -> (Uuid, &'a str, &'a HelperStartClaimV1) {
        match self {
            Self::Import(request) => (request.binding_ref, &request.namespace, &request.claim),
            Self::Verification(request) => {
                (request.binding_ref, &request.namespace, &request.claim)
            }
        }
    }

    fn input_hash(self) -> anyhow::Result<String> {
        match self {
            Self::Import(request) => request.input_hash(),
            Self::Verification(request) => request.input_hash(),
        }
    }

    fn validate_receipt(self, receipt: &MutationReceiptV1) -> anyhow::Result<()> {
        match self {
            Self::Import(request) => request.validate_receipt(receipt),
            Self::Verification(request) => request.validate_receipt(receipt),
        }
    }

    fn record(
        self,
        session_id: String,
        authorization_revision: u64,
        authorized_at: DateTime<Utc>,
    ) -> admission_codec::Input {
        match self {
            Self::Import(request) => {
                admission_codec::Input::LegacyNamedImportClaim(Box::new(LegacyNamedImportClaimV1 {
                    version: 1,
                    request: request.clone(),
                    session_id,
                    authorization_revision,
                    authorized_at,
                }))
            }
            Self::Verification(request) => {
                admission_codec::Input::LegacyNamedImportVerificationClaim(Box::new(
                    LegacyNamedImportVerificationClaimV2 {
                        version: 2,
                        request: request.clone(),
                        session_id,
                        authorization_revision,
                        authorized_at,
                    },
                ))
            }
        }
    }
}

fn named_claim_fact(
    input: &admission_codec::Input,
) -> anyhow::Result<(ClaimRequest<'_>, &str, u64, DateTime<Utc>)> {
    match input {
        admission_codec::Input::LegacyNamedImportClaim(claim) => Ok((
            ClaimRequest::Import(&claim.request),
            &claim.session_id,
            claim.authorization_revision,
            claim.authorized_at,
        )),
        admission_codec::Input::LegacyNamedImportVerificationClaim(claim) => Ok((
            ClaimRequest::Verification(&claim.request),
            &claim.session_id,
            claim.authorization_revision,
            claim.authorized_at,
        )),
        _ => Err(ControlFailure::Conflict.into()),
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
    claim_arm_record_at(db, ClaimRequest::Import(request), admission, before_seq)
        .map(|(_, arm)| arm)
}

fn claim_arm_record_at(
    db: &Connection,
    request: ClaimRequest<'_>,
    admission: &StoredEvent,
    before_seq: i64,
) -> anyhow::Result<(StoredEvent, LegacyNamedArmV1)> {
    let (binding_ref, namespace, claim) = request.parts();
    let receipt = &claim.arm_receipt;
    let kind = match request {
        ClaimRequest::Import(_) => LEGACY_NAMED_ARM_KIND,
        ClaimRequest::Verification(_) => LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND,
    };
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events \
             WHERE seq=?1 AND seq<?2 AND kind=?3",
            params![receipt.audit_seq, before_seq, kind],
            row_event,
        )
        .optional()?
        .ok_or(ControlFailure::NotFound)?;
    let record = admission_codec::decode(&stored)?;
    let arm = match (request, record.input) {
        (ClaimRequest::Import(_), admission_codec::Input::LegacyNamedArm(arm))
        | (
            ClaimRequest::Verification(_),
            admission_codec::Input::LegacyNamedImportVerificationArm(arm),
        ) => arm,
        _ => return Err(ControlFailure::Conflict.into()),
    };
    anyhow::ensure!(
        arm.binding_ref == binding_ref
            && arm.namespace == namespace
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
    match request {
        ClaimRequest::Import(request) => {
            request.validate_for_arm(arm.binding_ref, &arm.namespace, &arm.arm, &actual)?;
        }
        ClaimRequest::Verification(request) => {
            request.validate_for_arm(arm.binding_ref, &arm.namespace, &arm.arm, &actual)?;
        }
    }
    Ok((stored, *arm))
}

pub(in crate::audit) fn legacy_named_import_claim_predecessor(
    db: &Connection,
    stored: &StoredEvent,
    input: &admission_codec::Input,
    parent: &WorkerV1,
) -> anyhow::Result<()> {
    let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    conflict(format_admits(version))?;
    let (request, session_id, authorization_revision, authorized_at) = named_claim_fact(input)?;
    let (binding_ref, _, _) = request.parts();
    let (admission, fact) = claim_admission(db, binding_ref)?;
    let (arm_event, arm) = claim_arm_record_at(db, request, &admission, stored.seq)?;
    if matches!(request, ClaimRequest::Verification(_)) {
        Store::validate_named_import_verification_arm_predecessor(db, &arm_event, &arm, parent)?;
    }
    let (confirmed, confirmation, prepared) = admission_origin(db, binding_ref)?;
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
        stored.event.session_id.as_deref() == Some(session_id)
            && stored.event.coverage == "observed"
            && session_id == arm.session_id
            && session_id == parent.launch.session_id
            && parent.admission_operation_id == fact.admission.operation_id
            && arm.namespace == supervision::namespace(db)?
            && confirmed.hash == fact.confirmation_hash
            && target_event.hash == prepared.target_audit_hash
            && authorized_at >= arm.authorized_at
            && authorized_at <= crate::broker_registry::parse_time(&stored.received_at)?
            && authorized_at < authority.expires_at
            && authorization_revision >= arm.authorization_revision
            && authorization_revision == identity::authorization_revision_before(db, stored.seq)?,
    )?;
    conflict(target_broker_at(db, &target.command, authorized_at, stored.seq)?.is_some())?;
    workload::validate_broker_identity_at(
        db,
        &fact.actor.historical_identity()?,
        authorized_at,
        stored.seq,
    )
}

pub(in crate::audit) fn validate_named_import_claim_initiator(
    db: &Connection,
    stored: &StoredEvent,
    projection: &Projection,
) -> anyhow::Result<()> {
    let record = admission_codec::decode(stored)?;
    let (request, _, _, authorized_at) = named_claim_fact(&record.input)?;
    let (binding_ref, _, _) = request.parts();
    let (_, fact) = claim_admission(db, binding_ref)?;
    let (_, confirmation, prepared) = admission_origin(db, binding_ref)?;
    let initiator = &prepared.initiator;
    projection.require_local_launch_at(
        &initiator.principal,
        (&initiator.credential_id, initiator.credential_revision),
        (&initiator.membership_id, initiator.membership_revision),
        (&fact.actor.tenant_id, &initiator.organization),
        (stored.seq, authorized_at),
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
        request: ClaimRequest<'_>,
    ) -> anyhow::Result<AuthorizedClaimLookup> {
        Self::require_legacy_named_admission_format(tx)?;
        let clock = workload::capture_control_time(tx)?;
        let broker_deadline = workload::broker_identity_deadline_checked(tx, broker, clock.at())?;
        let (binding_ref, namespace, _) = request.parts();
        let (admission, fact) = claim_admission(tx, binding_ref)?;
        anyhow::ensure!(
            fact.actor.matches_binding(broker) && namespace == supervision::namespace(tx)?,
            ControlFailure::NotFound
        );
        let before_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let (_, arm) = claim_arm_record_at(tx, request, &admission, before_seq)?;
        Ok(AuthorizedClaimLookup {
            clock,
            broker_deadline,
            arm: arm.arm,
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
        self.claim_named_import_helper(broker, ClaimRequest::Import(request))
    }

    /// Consume a verification start claim for the original broker.
    #[allow(
        dead_code,
        reason = "the private verification claim route is composed separately"
    )]
    pub(crate) fn claim_legacy_named_import_verification(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportVerificationClaimRequestV2,
    ) -> anyhow::Result<HelperClaimResultV1> {
        self.claim_named_import_helper(broker, ClaimRequest::Verification(request))
    }

    fn claim_named_import_helper(
        &self,
        broker: &BrokerIdentityV1,
        request: ClaimRequest<'_>,
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
        let (binding_ref, namespace, claim) = request.parts();
        conflict(!named_operation(&tx, canonical_uuid(&claim.operation_id)?)?)?;
        let arm = &lookup.arm;
        let parent = control_store::worker_at(&tx, &arm.worker_id, lookup.before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        arm.validate_for_parent(&parent)?;
        let (human_deadline, broker_deadline) = self.require_legacy_named_execution(
            &tx,
            broker,
            binding_ref,
            namespace,
            &parent,
            &lookup.clock,
        )?;
        let input = request.record(
            parent.launch.session_id.clone(),
            identity::authorization_revision(&tx)?,
            lookup.clock.at(),
        );
        admission_codec::validate_input(&input, &self.redactor)?;
        let record = admission_codec::Record::new(input, None);
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
        let receipt = admission_codec::receipt_for(&stored, &record)?;
        request.validate_receipt(&receipt)?;
        self.commit_legacy_named_control(tx, &lookup.clock, |tx, current| {
            anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
            let (seq, hash) = audit_head(tx)?;
            conflict(seq == stored.seq && hash == stored.hash)?;
            let (human, broker) = self.require_legacy_named_execution(
                tx,
                broker,
                binding_ref,
                namespace,
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
        self.read_named_import_helper_claim(broker, ClaimRequest::Import(request))
    }

    /// Reconcile only the exact verification claim under the original broker.
    #[allow(
        dead_code,
        reason = "the private verification claim reconciliation route is composed separately"
    )]
    pub(crate) fn read_legacy_named_import_verification_claim(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedImportVerificationClaimRequestV2,
    ) -> anyhow::Result<Option<HelperClaimResultV1>> {
        self.read_named_import_helper_claim(broker, ClaimRequest::Verification(request))
    }

    fn read_named_import_helper_claim(
        &self,
        broker: &BrokerIdentityV1,
        request: ClaimRequest<'_>,
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
    request: ClaimRequest<'_>,
    input_hash: &str,
) -> anyhow::Result<Option<HelperClaimResultV1>> {
    let (binding_ref, namespace, claim) = request.parts();
    let Some((stored, record)) = supervision::load_record(db, &claim.operation_id)? else {
        return Ok(None);
    };
    let (retained, _, _, _) = named_claim_fact(&record.input)?;
    conflict(matches!(
        (request, retained),
        (ClaimRequest::Import(_), ClaimRequest::Import(_))
            | (ClaimRequest::Verification(_), ClaimRequest::Verification(_))
    ))?;
    let (retained_binding, retained_namespace, _) = retained.parts();
    anyhow::ensure!(
        retained_binding == binding_ref && retained_namespace == namespace,
        ControlFailure::NotFound
    );
    conflict(record.input.hash()? == input_hash)?;
    let receipt = admission_codec::receipt_for(&stored, &record)?;
    request.validate_receipt(&receipt)?;
    Ok(Some(HelperClaimResultV1 {
        receipt,
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
