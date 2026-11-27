//! One checked Legacy transaction for a confirmed admission and nominative grant.
use super::*;
use crate::{
    audit::{identity::projection::Projection, supervision_admission as admission_codec},
    control::{
        DecisionPhaseV1, MutationReceiptV1, QuotaPolicyV1, conflict,
        session_control::{
            LEGACY_NAMED_ADMISSION_REQUEST_BYTES, LegacyNamedAdmissionReceiptV1,
            LegacyNamedSupervisedAdmissionV1,
        },
    },
    identity::{
        IdentityChangeV1, IdentityMutationV1, IdentityReceiptV1, ResourceGrantV1, ResourceKindV1,
        ResourceRefV1,
    },
    workload::WorkloadFailureV1,
};

mod arm;
mod cancel;
mod execution;
mod initial_decision;
pub(crate) use arm::LegacyNamedArmV1;
pub(in crate::audit) use arm::{
    LEGACY_NAMED_ARM_KIND, legacy_named_arm_predecessor, validate_named_arm_initiator,
};
#[cfg(test)]
pub(in crate::audit) use cancel::LegacyNamedCancelReservationV1;
pub(crate) use cancel::LegacyNamedCancelV1;
pub(in crate::audit) use cancel::validate_legacy_named_cancel;
pub(crate) use initial_decision::LegacyNamedInitialDecision;

pub(in crate::audit) const ADMITTED: &str = "control.legacy_session_prepare.admission";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionActor {
    tenant_id: String,
    broker_id: String,
    credential_id: String,
    binding_revision: u64,
    grant_revision: u64,
}

impl AdmissionActor {
    fn capture(broker: &BrokerIdentityV1) -> Self {
        Self {
            tenant_id: broker.tenant_id().into(),
            broker_id: broker.broker_id().into(),
            credential_id: broker.credential_id().into(),
            binding_revision: broker.binding_revision(),
            grant_revision: broker.grant_revision(),
        }
    }

    fn matches_grant(&self, broker: &BrokerIdentityV1) -> bool {
        self.tenant_id == broker.tenant_id()
            && self.broker_id == broker.broker_id()
            && self.credential_id == broker.credential_id()
            && self.grant_revision == broker.grant_revision()
    }

    fn matches_binding(&self, broker: &BrokerIdentityV1) -> bool {
        self.matches_grant(broker) && self.binding_revision == broker.binding_revision()
    }

    fn historical_identity(&self) -> anyhow::Result<BrokerIdentityV1> {
        BrokerIdentityV1::authenticated(
            self.tenant_id.clone(),
            self.broker_id.clone(),
            self.credential_id.clone(),
            self.binding_revision,
            self.grant_revision,
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionFact {
    version: u32,
    binding_ref: Uuid,
    confirmation_hash: String,
    authorization_revision: u64,
    broker_profile_id: String,
    actor: AdmissionActor,
    input_hash: String,
    admission: MutationReceiptV1,
    grant: Option<IdentityReceiptV1>,
}

fn fact_id(binding_ref: Uuid) -> String {
    key("ai-guard:legacy-named-admission-result:v1", binding_ref)
}

fn session_owner_grant(tenant: &str, initiator: &Initiator, session_id: Uuid) -> ResourceGrantV1 {
    ResourceGrantV1 {
        version: 1,
        tenant_id: tenant.into(),
        organization_scope: initiator.organization.clone(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: session_id.to_string(),
        },
        owner: initiator.principal.clone(),
        revision: 1,
        active: true,
        shares: vec![],
    }
}

fn session_grant(
    tenant: &str,
    initiator: &Initiator,
    session_id: Uuid,
    binding_ref: Uuid,
) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: key("ai-guard:legacy-named-session-grant:v1", binding_ref),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "session-launch-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change: IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: session_owner_grant(tenant, initiator, session_id),
        },
    }
}

fn admission_record(
    db: &Connection,
    fact: &AdmissionFact,
) -> anyhow::Result<(StoredEvent, admission_codec::Record)> {
    let admission = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [fact.admission.audit_seq],
        row_event,
    )?;
    let record = admission_codec::decode(&admission)?;
    record.admission()?;
    conflict(admission_codec::receipt_for(&admission, &record)? == fact.admission)?;
    Ok((admission, record))
}

/// A completed admission under the fenced operation must have its retained fact.
/// Lookup uses the existing operation and session projections.
pub(super) fn require_link_when_admitted(
    db: &Connection,
    confirmed: &StoredEvent,
    record: &Confirmed,
) -> anyhow::Result<()> {
    let operation = record.request.operation_id.to_string();
    let worker_operation: Option<String> = db
        .query_row(
            "SELECT admission_operation_id FROM workers WHERE session_id=?1",
            [record.session_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    conflict(worker_operation.is_none_or(|retained| retained == operation))?;
    let admission_seq: Option<i64> = db
        .query_row(
            "SELECT event_seq FROM worker_mutations WHERE operation_id=?1
             AND action IN ('admit','supervised_admit') AND event_seq>?2",
            params![operation, confirmed.seq],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(seq) = admission_seq {
        let binding_ref = canonical_uuid(&confirmed.event.id)?;
        let linked =
            event_by_id(db, &fact_id(binding_ref))?.context("missing named admission fact")?;
        let fact: AdmissionFact = decoded(&linked, ADMITTED)?;
        conflict(fact.binding_ref == binding_ref && fact.admission.audit_seq == seq)?;
    }
    Ok(())
}

fn admission_origin(
    db: &Connection,
    binding_ref: Uuid,
) -> anyhow::Result<(StoredEvent, Confirmed, Prepared)> {
    let confirmed = event_by_id(db, &binding_ref.to_string())?
        .context("missing Legacy named admission confirmation")?;
    let confirmation: Confirmed = decoded(&confirmed, CONFIRMED)?;
    let preparation =
        event_by_id(db, &confirmation.preparation_id)?.context("missing Legacy preparation")?;
    let prepared = decoded(&preparation, PREPARED)?;
    Ok((confirmed, confirmation, prepared))
}

pub(in crate::audit) fn validate_admission_initiator(
    db: &Connection,
    stored: &StoredEvent,
    projection: &Projection,
) -> anyhow::Result<()> {
    let fact: AdmissionFact = decoded(stored, ADMITTED)?;
    let (_, _, prepared) = admission_origin(db, fact.binding_ref)?;
    let initiator = &prepared.initiator;
    // The structural replay enforces adjacency. The intervening first session
    // grant leaves the account, credential, membership and revision unchanged.
    projection.require_local_launch_at(
        &initiator.principal,
        (&initiator.credential_id, initiator.credential_revision),
        (&initiator.membership_id, initiator.membership_revision),
        (&fact.actor.tenant_id, &initiator.organization),
        (
            fact.admission.audit_seq,
            crate::broker_registry::parse_time(&stored.event.timestamp)?,
        ),
    )
}

pub(super) fn validate_fact(db: &Connection, stored: &StoredEvent) -> anyhow::Result<()> {
    let fact: AdmissionFact = decoded(stored, ADMITTED)?;
    let (admission, record) = admission_record(db, &fact)?;
    let (supervised, worker_record) = record.admission()?;
    let (confirmed, confirmation, prepared) = admission_origin(db, fact.binding_ref)?;
    let authority = confirmation
        .admission_authority
        .as_ref()
        .filter(|_| confirmation.version == 2)
        .context("missing Legacy named admission authority")?;
    let (target_event, target) = require_target_at(
        db,
        confirmation.request.selection()?,
        &prepared.initiator,
        admission.seq,
    )?;
    let selected = &target.command.target;
    // The linked fact retains the authorization observation made before
    // staging; the transaction separately checks the real deadline at commit.
    let at = crate::broker_registry::parse_time(&stored.event.timestamp)?;
    conflict(
        fact.version == 1
            && stored.event.id == fact_id(fact.binding_ref)
            && fact.confirmation_hash == confirmed.hash
            && confirmed.seq < admission.seq
            && target_event.hash == prepared.target_audit_hash
            && at < authority.expires_at
            && fact.authorization_revision >= authority.authorization_revision
            && fact.authorization_revision
                == identity::authorization_revision_before(db, admission.seq)?
            && fact.actor.tenant_id == selected.catalogue.tenant_id
            && fact.actor.broker_id == selected.broker_id.to_string()
            && fact.actor.grant_revision == selected.broker_grant_revision,
    )?;
    conflict(target_broker_at(db, &target.command, at, admission.seq)?.is_some())?;
    workload::validate_broker_identity_at(
        db,
        &fact.actor.historical_identity()?,
        at,
        admission.seq,
    )?;

    let request = LegacyNamedSupervisedAdmissionV1 {
        version: 1,
        binding_ref: fact.binding_ref,
        broker_profile_id: fact.broker_profile_id.clone(),
        admission: supervised.clone(),
    };
    let (plan, _) = confirmed_plan(
        fact.binding_ref,
        &confirmation,
        &prepared,
        target,
        authority.expires_at,
    )?;
    request.require_plan(&plan)?;
    conflict(
        request.input_hash()? == fact.input_hash
            && supervised.admission.receipt.phase == DecisionPhaseV1::Initial,
    )?;
    for kind in [FenceKind::Operation, FenceKind::Session] {
        let fence = event_by_id(db, &fence_id(kind, &confirmation))?.context("missing fence")?;
        conflict(fence.seq < admission.seq)?;
    }
    match (&worker_record.worker, &fact.grant) {
        (Some(_), Some(grant)) => {
            conflict(
                admission.seq.checked_add(1) == Some(grant.audit_seq)
                    && grant.audit_seq.checked_add(1) == Some(stored.seq),
            )?;
            identity::require_recorded_mutation(
                db,
                &session_grant(
                    &fact.actor.tenant_id,
                    &prepared.initiator,
                    confirmation.session_id,
                    fact.binding_ref,
                ),
                grant,
            )?;
        }
        (None, None) => {
            conflict(
                worker_record.refusal.is_some() && admission.seq.checked_add(1) == Some(stored.seq),
            )?;
        }
        _ => return Err(ControlFailure::Conflict.into()),
    }
    Ok(())
}

impl Store {
    fn verify_legacy_named_admission_input(
        &self,
        request: &LegacyNamedSupervisedAdmissionV1,
    ) -> anyhow::Result<String> {
        let input_hash = request.input_hash()?;
        let value = serde_json::to_value(request)?;
        crate::control::invalid(
            self.redactor.value(&value) == value
                && serde_json::to_vec(&value)?.len() <= LEGACY_NAMED_ADMISSION_REQUEST_BYTES,
        )?;
        Ok(input_hash)
    }

    fn require_legacy_named_admission_format(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
        let version = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            format_admits(version) && capability_present(tx)?,
            ControlFailure::MigrationRequired
        );
        Ok(())
    }

    fn retained_legacy_named_admission(
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedSupervisedAdmissionV1,
        input_hash: &str,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<Option<LegacyNamedAdmissionReceiptV1>> {
        let Some(stored) = event_by_id(tx, &fact_id(request.binding_ref))? else {
            return Ok(None);
        };
        workload::revalidate_broker_identity_checked(tx, broker, clock.at())?;
        let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
        anyhow::ensure!(fact.actor.matches_grant(broker), ControlFailure::NotFound);
        conflict(fact.binding_ref == request.binding_ref && fact.input_hash == input_hash)?;
        let (admission, record) = admission_record(tx, &fact)?;
        Ok(Some(LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: admission_codec::result(&admission, &record, true)?,
            grant: fact.grant,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        }))
    }

    fn resolve_fresh_legacy_named_admission(
        &self,
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedSupervisedAdmissionV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<ResolvedLegacyConfirmedPlan> {
        let resolved =
            self.resolve_legacy_confirmed_plan(tx, broker, request.binding_ref, clock)?;
        request.require_plan(&resolved.plan)?;
        anyhow::ensure!(tx.lease.active, ControlFailure::SupervisionInactive);
        // The launch confirmation does not approve a separate policy review.
        let admission = &request.admission.admission;
        conflict(admission.receipt.phase == DecisionPhaseV1::Initial)?;
        initial_decision::require_named_origin_when_used(tx, broker, &resolved, admission)?;
        conflict(supervision::operation_receipt(tx, &admission.operation_id)?.is_none())?;
        Ok(resolved)
    }

    /// Resolve a retained receipt. With a quota policy, also check eligibility
    /// for a fresh request before policy evaluation.
    pub(crate) fn read_legacy_named_admission(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedSupervisedAdmissionV1,
        fresh_quota: Option<&QuotaPolicyV1>,
    ) -> anyhow::Result<Option<LegacyNamedAdmissionReceiptV1>> {
        let input_hash = self.verify_legacy_named_admission_input(request)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        Self::require_legacy_named_admission_format(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        if let Some(receipt) =
            Self::retained_legacy_named_admission(&tx, broker, request, &input_hash, &clock)?
        {
            let deadline = workload::broker_identity_deadline_checked(&tx, broker, clock.at())?;
            self.commit_authorized(tx, &clock)?;
            anyhow::ensure!(
                workload::canonical_control_time(Utc::now())? < deadline,
                WorkloadFailureV1::Unauthorized
            );
            return Ok(Some(receipt));
        }
        let Some(quota) = fresh_quota else {
            // Absence is meaningful only for this broker's exact, current plan.
            // Keep all other references indistinguishable from a foreign receipt.
            let resolved =
                self.resolve_legacy_confirmed_plan(&tx, broker, request.binding_ref, &clock)?;
            request.require_plan(&resolved.plan)?;
            self.commit_authorized(tx, &clock)?;
            resolved.release_at(Utc::now())?;
            return Ok(None);
        };
        quota.validate()?;
        let resolved = self.resolve_fresh_legacy_named_admission(&tx, broker, request, &clock)?;
        let admission = &request.admission.admission;
        let stored = Self::lookup_decision(
            &tx,
            &admission.decision_request,
            &admission.receipt.request_hash,
        )?
        .ok_or(ControlFailure::InvalidRequest)?;
        crate::audit::worker_admission::require_receipt(&admission.receipt, &stored)?;
        self.commit_authorized(tx, &clock)?;
        resolved.release_at(Utc::now())?;
        Ok(None)
    }

    /// Commit a Legacy named reservation on the authenticated broker's Store.
    /// The retained fact also supports technical replay on an inactive reopen.
    pub(crate) fn admit_legacy_named(
        &self,
        broker: &BrokerIdentityV1,
        request: &LegacyNamedSupervisedAdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<LegacyNamedAdmissionReceiptV1> {
        let input_hash = self.verify_legacy_named_admission_input(request)?;
        let input = admission_codec::admission_input(&request.admission, quota)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        Self::require_legacy_named_admission_format(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        let event_id = fact_id(request.binding_ref);
        if let Some(receipt) =
            Self::retained_legacy_named_admission(&tx, broker, request, &input_hash, &clock)?
        {
            self.commit_legacy_named_control(tx, &clock, |tx, current| {
                Ok((
                    None,
                    workload::broker_identity_deadline_checked(tx, broker, current.at())?,
                ))
            })?;
            return Ok(receipt);
        }
        let resolved = self.resolve_fresh_legacy_named_admission(&tx, broker, request, &clock)?;
        let worker_record = self.build_admission_record(
            &tx,
            &request.admission.admission,
            policy,
            fresh_decision,
            jev,
            quota,
        )?;
        let accepted = worker_record.worker.is_some();
        let record = admission_codec::Record::new(input, Some(worker_record));
        let admission = self.persist_supervision_record(&tx, &record)?;
        let result = admission_codec::result(&admission, &record, false)?;
        let grant = if accepted {
            let receipt = self.persist_identity_mutation(
                &tx,
                &session_grant(
                    &self.tenant_id,
                    &resolved.initiator,
                    resolved.plan.session_id,
                    request.binding_ref,
                ),
            )?;
            conflict(admission.seq.checked_add(1) == Some(receipt.audit_seq))?;
            Some(receipt)
        } else {
            None
        };
        let fact = AdmissionFact {
            version: 1,
            binding_ref: request.binding_ref,
            confirmation_hash: resolved.confirmed.hash,
            authorization_revision: resolved.authorization_revision,
            broker_profile_id: request.broker_profile_id.clone(),
            actor: AdmissionActor::capture(broker),
            input_hash,
            admission: result.receipt.clone(),
            grant,
        };
        let stored = self.append_legacy_session_record(&tx, &clock, ADMITTED, event_id, &fact)?;
        self.validate_supervision_write(&tx)?;
        self.commit_legacy_named_control(tx, &clock, |tx, current| {
            if let Some(worker) = &result.worker {
                let (human_deadline, broker_deadline) = self.require_legacy_named_execution(
                    tx,
                    broker,
                    request.binding_ref,
                    &resolved.plan.control_namespace_uuid,
                    worker,
                    current,
                )?;
                Ok((Some(human_deadline), broker_deadline))
            } else {
                let resolved =
                    self.resolve_legacy_confirmed_plan(tx, broker, request.binding_ref, current)?;
                request.require_plan(&resolved.plan)?;
                Ok((Some(resolved.plan.expires_at), resolved.broker_deadline))
            }
        })?;
        Ok(LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: result,
            grant: fact.grant,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        })
    }

    fn commit_legacy_named_control(
        &self,
        tx: CheckedTransaction<'_>,
        clock: &workload::AuthorizedControlTimeV1,
        authorize: impl FnOnce(
            &CheckedTransaction<'_>,
            &workload::AuthorizedControlTimeV1,
        ) -> anyhow::Result<(Option<DateTime<Utc>>, DateTime<Utc>)>,
    ) -> anyhow::Result<()> {
        self.commit_checked_guarded(tx, |tx| {
            let current = workload::capture_control_time(tx)?;
            let (human_deadline, broker_deadline) = authorize(tx, &current)?;
            let validated_at = Utc::now();
            workload::validate_fresh_control_time_at(tx, clock, validated_at)?;
            let now = Utc::now();
            #[cfg(test)]
            let now = self
                .named_control_commit_observation
                .lock()
                .map_err(|_| IdentityFailureV1::IdentityUnavailable)?
                .take()
                .unwrap_or(now);
            anyhow::ensure!(now >= validated_at, WorkloadFailureV1::ClockRegression);
            anyhow::ensure!(
                human_deadline.is_none_or(|deadline| now < deadline),
                ControlFailure::NotFound
            );
            anyhow::ensure!(
                workload::canonical_control_time(now)? < broker_deadline,
                WorkloadFailureV1::Unauthorized
            );
            Ok(())
        })
    }
}
