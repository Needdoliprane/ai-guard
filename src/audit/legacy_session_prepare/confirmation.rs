use super::*;
use crate::{
    broker_registry::BrokerIdentityV1,
    control::{
        ControlFailure,
        session_control::{
            LegacyConfirmedPlanV1, LegacySessionConfirmationReceiptV1,
            SessionLaunchPrepareRequestV1, SessionLaunchRequestV1, SessionPrepareReceiptV1,
            canonical_uuid,
        },
    },
    identity::{AuthorizedScopeV1, ContextModeV1, HumanCapabilityV1, LocalCredentialV1},
};
use chrono::{DateTime, Utc};

mod admission;
#[cfg(test)]
mod tests;

const PREPARED: &str = "control.legacy_session_prepare.prepared";
const CONFIRMED: &str = "control.legacy_session_prepare.confirmed";
const FENCED: &str = "control.legacy_session_prepare.fenced";
pub(in crate::audit) const REFERENCE_KINDS: [&str; 4] =
    [PREPARED, CONFIRMED, FENCED, admission::ADMITTED];

/// Borrowed proof from the authenticated local gateway context. The digest
/// is used solely to re-read the credential inside the write transaction.
pub(crate) struct LegacySessionLocalProof<'a> {
    pub scope: &'a AuthorizedScopeV1,
    pub digest: &'a [u8; 32],
    pub credential_id: &'a str,
    pub credential_revision: u64,
    pub context_expires_at: &'a str,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Initiator {
    principal: PrincipalRefV1,
    credential_id: String,
    credential_revision: u64,
    membership_id: String,
    membership_revision: u64,
    authorization_revision: u64,
    organization: String,
}

impl Initiator {
    fn same_binding(&self, current: &Self) -> bool {
        // The tenant's authorization revision is retained for provenance and
        // checked on the presented scope before comparing the durable binding.
        self.principal == current.principal
            && self.credential_id == current.credential_id
            && self.credential_revision == current.credential_revision
            && self.membership_id == current.membership_id
            && self.membership_revision == current.membership_revision
            && self.organization == current.organization
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    version: u32,
    namespace: String,
    initiator: Initiator,
    request: SessionLaunchPrepareRequestV1,
    target_audit_hash: String,
    launch_template_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmed {
    version: u32,
    request: SessionLaunchRequestV1,
    preparation_id: String,
    preparation_audit_hash: String,
    session_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_authority: Option<ConfirmationAuthority>,
}

/// The authorization limits observed at confirmation. Replaying the public
/// receipt can use a fresh context, but cannot renew this retained authority.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmationAuthority {
    expires_at: DateTime<Utc>,
    authorization_revision: u64,
}

struct ResolvedLegacyConfirmedPlan {
    plan: LegacyConfirmedPlanV1,
    confirmed: StoredEvent,
    initiator: Initiator,
}

fn confirmed_plan(
    binding_ref: Uuid,
    record: &Confirmed,
    prepared: &Prepared,
    target: TargetRecord,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<LegacyConfirmedPlanV1> {
    let target = target.command.target;
    let mut launch = target.launch;
    launch.session_id = record.session_id.to_string();
    Ok(LegacyConfirmedPlanV1 {
        version: 1,
        binding_ref,
        control_namespace_uuid: prepared.namespace.clone(),
        operation_id: record.request.operation_id,
        session_id: record.session_id,
        launch_target_id: target.catalogue.launch_target_id,
        target_revision: target.catalogue.revision,
        broker_profile_id: target.broker_profile_id,
        launch_spec_hash: launch.spec_hash()?,
        expires_at,
    })
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FenceKind {
    Operation,
    Session,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fence {
    version: u32,
    binding_ref: String,
    kind: FenceKind,
}

fn key(domain: &str, operation: Uuid) -> String {
    session_prepare::bound_event_id(domain.as_bytes(), operation)
}

fn prepare_id(operation: Uuid) -> String {
    key("ai-guard:legacy-session-prepare:v1", operation)
}

fn confirmation_ref(prepared: &StoredEvent, operation: Uuid) -> String {
    key(
        &format!("ai-guard:legacy-session-reference:v1:{}", prepared.hash),
        operation,
    )
}

fn session_id(operation: Uuid) -> anyhow::Result<Uuid> {
    canonical_uuid(&key("ai-guard:legacy-named-session:v1", operation))
}

fn fence_id(kind: FenceKind, confirmation: &Confirmed) -> String {
    match kind {
        FenceKind::Operation => operation_fence_id(confirmation.request.operation_id),
        FenceKind::Session => session_fence_id(confirmation.session_id),
    }
}

fn operation_fence_id(operation: Uuid) -> String {
    key("ai-guard:legacy-named-operation-fence:v1", operation)
}

fn session_fence_id(session: Uuid) -> String {
    key("ai-guard:legacy-named-session-fence:v1", session)
}

/// Occupancy check for new placements in another ledger.
pub(in crate::audit) fn session_is_fenced(
    tx: &CheckedTransaction<'_>,
    session: Uuid,
) -> anyhow::Result<bool> {
    Ok(event_by_id(tx, &session_fence_id(session))?
        .is_some_and(|stored| stored.event.kind == FENCED))
}

fn target(
    db: &Connection,
    selection: crate::control::session_control::SessionLaunchTargetSelectionV1,
) -> anyhow::Result<(StoredEvent, TargetRecord)> {
    let id = target_event_id(
        selection.launch_target_id(),
        selection.expected_target_revision(),
    );
    let stored = event_by_id(db, &id)?.ok_or(IdentityFailureV1::NotFound)?;
    anyhow::ensure!(
        stored.event.kind == TARGET_KIND,
        IdentityFailureV1::Conflict
    );
    let record = serde_json::from_value(stored.event.data.clone())?;
    Ok((stored, record))
}

fn decoded<T: serde::de::DeserializeOwned>(
    stored: &StoredEvent,
    expected_kind: &str,
) -> anyhow::Result<T> {
    anyhow::ensure!(
        stored.event.kind == expected_kind,
        IdentityFailureV1::Conflict
    );
    Ok(serde_json::from_value(stored.event.data.clone())?)
}

fn require_target_launcher(record: &TargetRecord, initiator: &Initiator) -> anyhow::Result<()> {
    let catalogue = &record.command.target.catalogue;
    anyhow::ensure!(
        catalogue.active
            && catalogue.organization_scope == initiator.organization
            && catalogue.launchers.contains(&initiator.principal),
        IdentityFailureV1::PermissionDenied
    );
    Ok(())
}

fn require_current_target(
    db: &Connection,
    selection: crate::control::session_control::SessionLaunchTargetSelectionV1,
    initiator: &Initiator,
    clock: &workload::AuthorizedControlTimeV1,
) -> anyhow::Result<(StoredEvent, TargetRecord)> {
    let (stored, record) = target(db, selection)?;
    require_target_launcher(&record, initiator)?;
    let catalogue = &record.command.target.catalogue;
    anyhow::ensure!(
        event_by_id(
            db,
            &target_event_id(catalogue.launch_target_id, catalogue.revision + 1)
        )?
        .is_none(),
        IdentityFailureV1::Conflict
    );
    validate_target_broker(db, &record.command, clock.at(), audit_head(db)?.0 + 1)?;
    Ok((stored, record))
}

fn authorize_target_replay(
    db: &Connection,
    preparation: &Prepared,
    initiator: &Initiator,
) -> anyhow::Result<()> {
    let selection = preparation.request.selection()?;
    let (pinned, _) = target(db, selection)?;
    anyhow::ensure!(
        pinned.hash == preparation.target_audit_hash,
        IdentityFailureV1::Conflict
    );
    // Provisioning enforces contiguous revisions in audit order for each target.
    let current = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events \
         WHERE kind=?1 AND seq>=?2 AND json_extract(payload,'$.data.command.target.catalogue.launch_target_id')=?3 \
         ORDER BY seq DESC LIMIT 1",
        params![TARGET_KIND, pinned.seq, selection.launch_target_id().to_string()],
        row_event,
    )?;
    let record: TargetRecord = decoded(&current, TARGET_KIND)?;
    require_target_launcher(&record, initiator)
}

fn prepared_receipt(
    stored: &StoredEvent,
    record: &Prepared,
    replayed: bool,
) -> anyhow::Result<SessionPrepareReceiptV1> {
    let request = &record.request;
    Ok(SessionPrepareReceiptV1 {
        version: 1,
        operation_id: request.operation_id,
        preparation_id: stored.event.id.clone(),
        launch_target_id: request.launch_target_id,
        target_revision: request.expected_target_revision,
        input_hash: request.input_hash()?,
        expires_at: request.expires_at.clone(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
        replayed,
    })
}

fn confirmed_receipt(
    stored: &StoredEvent,
    record: &Confirmed,
    replayed: bool,
) -> LegacySessionConfirmationReceiptV1 {
    LegacySessionConfirmationReceiptV1 {
        version: 1,
        operation_id: record.request.operation_id,
        preparation_operation_id: record.request.preparation_operation_id,
        binding_ref: stored.event.id.clone(),
        session_id: record.session_id.to_string(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
        replayed,
    }
}

pub(in crate::audit) fn protect_manual(
    tx: &Connection,
    operation: &str,
    session: &str,
) -> anyhow::Result<()> {
    if !capability_present(tx)? {
        return Ok(());
    }
    let protected = named_operation(tx, canonical_uuid(operation)?)?
        || event_by_id(tx, &session_fence_id(canonical_uuid(session)?))?.is_some();
    anyhow::ensure!(!protected, ControlFailure::Conflict);
    Ok(())
}

fn named_operation(tx: &Connection, operation: Uuid) -> anyhow::Result<bool> {
    Ok(event_by_id(tx, &prepare_id(operation))?.is_some()
        || event_by_id(tx, &operation_fence_id(operation))?.is_some())
}

fn matching_prepare(record: &Prepared, request: &SessionLaunchRequestV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        record.request.operation_id == request.preparation_operation_id
            && record.request.launch_target_id == request.launch_target_id
            && record.request.expected_target_revision == request.expected_target_revision
            && record.request.input_hash()? == request.preparation_input_hash,
        IdentityFailureV1::Conflict
    );
    Ok(())
}

fn plan_lookup_failure(error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<IdentityFailureV1>() {
        Some(
            IdentityFailureV1::NotFound
            | IdentityFailureV1::PermissionDenied
            | IdentityFailureV1::IdentityRevoked
            | IdentityFailureV1::ContextRevoked
            | IdentityFailureV1::Conflict,
        ) => ControlFailure::NotFound.into(),
        _ => error,
    }
}

pub(in crate::audit) fn validate_reference_event(
    tx: &Connection,
    version: i64,
    stored: &StoredEvent,
) -> anyhow::Result<()> {
    let event = &stored.event;
    let marker: i64 = tx.query_row(
        "SELECT genesis_seq FROM legacy_session_prepare_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        format_admits(version)
            && marker < stored.seq
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "native"
            && event.session_id.is_none(),
        "invalid Legacy session reference provenance"
    );
    match event.kind.as_str() {
        PREPARED => {
            let record: Prepared = decoded(stored, PREPARED)?;
            let (target_event, target) = target(tx, record.request.selection()?)?;
            record.request.validate()?;
            record.initiator.principal.validate()?;
            canonical_uuid(&record.initiator.credential_id)?;
            canonical_uuid(&record.initiator.membership_id)?;
            let at = crate::broker_registry::parse_time(&event.timestamp)?;
            anyhow::ensure!(
                record.version == 1
                    && matches!(
                        record.initiator.principal,
                        PrincipalRefV1::HumanLocal { .. }
                    )
                    && record.initiator.credential_revision > 0
                    && record.initiator.membership_revision > 0
                    && record.namespace == supervision::namespace(tx)?
                    && event.id == prepare_id(record.request.operation_id)
                    && record.target_audit_hash == target_event.hash
                    && target_event.seq < stored.seq
                    && target.command.target.catalogue.active
                    && target
                        .command
                        .target
                        .catalogue
                        .launchers
                        .contains(&record.initiator.principal)
                    && record.initiator.organization
                        == target.command.target.catalogue.organization_scope
                    && crate::broker_registry::parse_time(&record.request.expires_at)? > at
                    && record.launch_template_hash == target.command.target.launch.spec_hash()?,
                "invalid Legacy preparation"
            );
        }
        CONFIRMED => {
            let record: Confirmed = decoded(stored, CONFIRMED)?;
            record.request.validate()?;
            let prepared =
                event_by_id(tx, &record.preparation_id)?.context("missing preparation")?;
            let preparation: Prepared = decoded(&prepared, PREPARED)?;
            matching_prepare(&preparation, &record.request)?;
            let confirmed_at = crate::broker_registry::parse_time(&event.timestamp)?;
            let prepare_deadline =
                crate::broker_registry::parse_time(&preparation.request.expires_at)?;
            let valid_authority = match (record.version, record.admission_authority.as_ref()) {
                (1, None) => true,
                (2, Some(authority)) => {
                    authority.expires_at > confirmed_at
                        && authority.expires_at <= prepare_deadline
                        && authority.authorization_revision
                            >= preparation.initiator.authorization_revision
                        && authority.authorization_revision
                            == identity::authorization_revision_before(tx, stored.seq)?
                }
                _ => false,
            };
            anyhow::ensure!(
                valid_authority
                    && prepared.seq < stored.seq
                    && prepared.hash == record.preparation_audit_hash
                    && event.id == confirmation_ref(&prepared, preparation.request.operation_id)
                    && record.session_id == session_id(record.request.operation_id)?
                    && prepare_deadline > confirmed_at,
                "invalid Legacy confirmation"
            );
            for kind in [FenceKind::Operation, FenceKind::Session] {
                let fence = event_by_id(tx, &fence_id(kind, &record))?
                    .context("missing confirmation fence")?;
                let payload: Fence = decoded(&fence, FENCED)?;
                anyhow::ensure!(
                    fence.seq > stored.seq && payload.binding_ref == event.id,
                    "invalid confirmation fence"
                );
            }
            admission::require_link_when_admitted(tx, stored, &record)?;
        }
        FENCED => {
            let fence: Fence = decoded(stored, FENCED)?;
            let confirmation =
                event_by_id(tx, &fence.binding_ref)?.context("missing confirmation")?;
            let record: Confirmed = decoded(&confirmation, CONFIRMED)?;
            anyhow::ensure!(
                fence.version == 1
                    && confirmation.seq < stored.seq
                    && event.id == fence_id(fence.kind, &record),
                "invalid Legacy confirmation fence"
            );
        }
        admission::ADMITTED => admission::validate_fact(tx, stored)?,
        _ => anyhow::bail!("unknown Legacy reference event"),
    }
    Ok(())
}

impl Store {
    pub(crate) fn legacy_confirmed_plan(
        &self,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
    ) -> anyhow::Result<LegacyConfirmedPlanV1> {
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
        let plan = self
            .resolve_legacy_confirmed_plan(&tx, broker, binding_ref, &clock)?
            .plan;
        self.commit_authorized(tx, &clock)?;
        anyhow::ensure!(plan.expires_at > Utc::now(), ControlFailure::NotFound);
        Ok(plan)
    }

    fn resolve_legacy_confirmed_plan(
        &self,
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<ResolvedLegacyConfirmedPlan> {
        workload::revalidate_broker_identity_checked(tx, broker, clock.at())?;
        let confirmed = event_by_id(tx, &binding_ref.to_string())?
            .filter(|stored| stored.event.kind == CONFIRMED)
            .ok_or(ControlFailure::NotFound)?;
        let record: Confirmed = decoded(&confirmed, CONFIRMED)?;
        let authority = record
            .admission_authority
            .as_ref()
            .filter(|authority| record.version == 2 && authority.expires_at > clock.at())
            .ok_or(ControlFailure::NotFound)?;
        let preparation =
            event_by_id(tx, &record.preparation_id)?.context("missing Legacy preparation")?;
        let prepared: Prepared = decoded(&preparation, PREPARED)?;
        let (target_event, target) =
            require_current_target(tx, record.request.selection()?, &prepared.initiator, clock)
                .map_err(plan_lookup_failure)?;
        anyhow::ensure!(
            target.command.target.catalogue.tenant_id == broker.tenant_id()
                && target.command.target.broker_id.to_string() == broker.broker_id()
                && target.command.target.broker_grant_revision == broker.grant_revision(),
            ControlFailure::NotFound
        );
        matching_prepare(&prepared, &record.request)?;
        anyhow::ensure!(
            preparation.hash == record.preparation_audit_hash
                && target_event.hash == prepared.target_audit_hash
                && prepared.namespace == supervision::namespace(tx)?
                && target.command.target.launch.spec_hash()? == prepared.launch_template_hash
                && record.session_id == session_id(record.request.operation_id)?,
            ControlFailure::Conflict
        );
        let initiator = &prepared.initiator;
        let credential: LocalCredentialV1 = identity::json_row(
            tx,
            "SELECT payload_json FROM identity_credentials WHERE credential_id=?1",
            &initiator.credential_id,
        )?
        .ok_or(ControlFailure::NotFound)?;
        credential.validate()?;
        anyhow::ensure!(
            credential.active
                && credential.credential_id == initiator.credential_id
                && credential.revision == initiator.credential_revision
                && credential.principal == initiator.principal,
            ControlFailure::NotFound
        );
        let expires_at = match credential.expires_at.as_deref() {
            Some(expiry) => authority
                .expires_at
                .min(crate::broker_registry::parse_time(expiry)?),
            None => authority.expires_at,
        };
        let scope = self
            .authorize_human_context_checked(
                tx,
                &initiator.principal,
                &initiator.principal.key()?,
                &initiator.membership_id,
                initiator.membership_revision,
                ContextModeV1::Personal,
            )
            .map_err(plan_lookup_failure)?;
        anyhow::ensure!(
            scope.has_capability(HumanCapabilityV1::LaunchSession)
                && scope.organization_scope() == initiator.organization
                && scope.authorization_revision() >= authority.authorization_revision
                && expires_at > clock.at(),
            ControlFailure::NotFound
        );
        let plan = confirmed_plan(binding_ref, &record, &prepared, target, expires_at)?;
        Ok(ResolvedLegacyConfirmedPlan {
            plan,
            confirmed,
            initiator: prepared.initiator,
        })
    }

    fn legacy_local_initiator(
        &self,
        tx: &CheckedTransaction<'_>,
        proof: &LegacySessionLocalProof<'_>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(Initiator, DateTime<Utc>)> {
        anyhow::ensure!(
            capability_present(tx)?,
            IdentityFailureV1::MigrationRequired
        );
        let context_expiry = crate::broker_registry::parse_time(proof.context_expires_at)?;
        let current = identity::revalidate_scope_checked(tx, proof.scope, &self.tenant_id)?;
        let credential = self
            .authenticate_local_digest_checked(tx, proof.digest)?
            .ok_or(IdentityFailureV1::IdentityRevoked)?;
        anyhow::ensure!(
            current.mode() == ContextModeV1::Personal
                && current.has_capability(HumanCapabilityV1::LaunchSession)
                && matches!(current.principal(), PrincipalRefV1::HumanLocal { .. })
                && current.principal() == &credential.principal
                && credential.credential_id == proof.credential_id
                && credential.revision == proof.credential_revision,
            IdentityFailureV1::PermissionDenied
        );
        let deadline = match credential.expires_at.as_deref() {
            Some(expiry) => context_expiry.min(crate::broker_registry::parse_time(expiry)?),
            None => context_expiry,
        };
        anyhow::ensure!(deadline > now, IdentityFailureV1::ContextRevoked);
        Ok((
            Initiator {
                principal: current.principal().clone(),
                credential_id: credential.credential_id,
                credential_revision: credential.revision,
                membership_id: current.membership_id().into(),
                membership_revision: current.membership_revision(),
                authorization_revision: current.authorization_revision(),
                organization: current.organization_scope().into(),
            },
            deadline,
        ))
    }

    fn append_legacy_session_record(
        &self,
        tx: &CheckedTransaction<'_>,
        clock: &workload::AuthorizedControlTimeV1,
        kind: &str,
        id: String,
        record: &impl Serialize,
    ) -> anyhow::Result<StoredEvent> {
        let mut event = Event::new(kind);
        event.id = id;
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.data = serde_json::to_value(record)?;
        let expected = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == expected,
            "Legacy reference record requires redaction"
        );
        self.append_with_authorized_capacity_class_at(
            tx,
            event,
            payload,
            capacity::Class::OrdinaryWrite,
            clock,
            Utc::now(),
        )
    }

    pub(crate) fn legacy_session_prepare_local(
        &self,
        proof: LegacySessionLocalProof<'_>,
        request: SessionLaunchPrepareRequestV1,
    ) -> anyhow::Result<SessionPrepareReceiptV1> {
        request
            .validate()
            .map_err(|_| IdentityFailureV1::InvalidRequest)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let clock = workload::capture_control_time(&tx)?;
        let (initiator, deadline) = self.legacy_local_initiator(&tx, &proof, clock.at())?;
        let expires = crate::broker_registry::parse_time(&request.expires_at)?;
        anyhow::ensure!(
            expires > clock.at() && expires <= deadline,
            IdentityFailureV1::InvalidRequest
        );
        let id = prepare_id(request.operation_id);
        if let Some(stored) = event_by_id(&tx, &id)? {
            let record: Prepared = decoded(&stored, PREPARED)?;
            anyhow::ensure!(
                record.initiator.same_binding(&initiator),
                IdentityFailureV1::PermissionDenied
            );
            anyhow::ensure!(record.request == request, IdentityFailureV1::Conflict);
            authorize_target_replay(&tx, &record, &initiator)?;
            let receipt = prepared_receipt(&stored, &record, true)?;
            self.commit_authorized(tx, &clock)?;
            anyhow::ensure!(expires > Utc::now(), IdentityFailureV1::InvalidRequest);
            return Ok(receipt);
        }
        let worker_operation: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_mutations WHERE operation_id=?1)",
            [request.operation_id.to_string()],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            !worker_operation && !named_operation(&tx, request.operation_id)?,
            IdentityFailureV1::Conflict
        );
        let (target, target_record) =
            require_current_target(&tx, request.selection()?, &initiator, &clock)?;
        let record = Prepared {
            version: 1,
            namespace: supervision::namespace(&tx)?,
            initiator,
            request,
            target_audit_hash: target.hash,
            launch_template_hash: target_record.command.target.launch.spec_hash()?,
        };
        let stored = self.append_legacy_session_record(&tx, &clock, PREPARED, id, &record)?;
        let receipt = prepared_receipt(&stored, &record, false)?;
        anyhow::ensure!(expires > Utc::now(), IdentityFailureV1::InvalidRequest);
        self.commit_authorized(tx, &clock)?;
        Ok(receipt)
    }

    pub(crate) fn legacy_session_confirm_local(
        &self,
        proof: LegacySessionLocalProof<'_>,
        request: SessionLaunchRequestV1,
    ) -> anyhow::Result<LegacySessionConfirmationReceiptV1> {
        request
            .validate()
            .map_err(|_| IdentityFailureV1::InvalidRequest)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
        let tx = self.begin_checked(&mut db)?;
        let clock = workload::capture_control_time(&tx)?;
        let (initiator, deadline) = self.legacy_local_initiator(&tx, &proof, clock.at())?;
        let preparation_id = prepare_id(request.preparation_operation_id);
        let stored = event_by_id(&tx, &preparation_id)?.ok_or(IdentityFailureV1::NotFound)?;
        let prepared: Prepared = decoded(&stored, PREPARED)?;
        anyhow::ensure!(
            prepared.initiator.same_binding(&initiator),
            IdentityFailureV1::PermissionDenied
        );
        matching_prepare(&prepared, &request)?;
        let id = confirmation_ref(&stored, request.preparation_operation_id);
        if let Some(confirmed) = event_by_id(&tx, &id)? {
            let record: Confirmed = decoded(&confirmed, CONFIRMED)?;
            anyhow::ensure!(record.request == request, IdentityFailureV1::Conflict);
            authorize_target_replay(&tx, &prepared, &initiator)?;
            let receipt = confirmed_receipt(&confirmed, &record, true);
            self.commit_authorized(tx, &clock)?;
            anyhow::ensure!(deadline > Utc::now(), IdentityFailureV1::ContextRevoked);
            return Ok(receipt);
        }
        let expires = deadline.min(crate::broker_registry::parse_time(
            &prepared.request.expires_at,
        )?);
        anyhow::ensure!(expires > clock.at(), IdentityFailureV1::InvalidRequest);
        let (target, _) = require_current_target(&tx, request.selection()?, &initiator, &clock)?;
        anyhow::ensure!(
            target.hash == prepared.target_audit_hash,
            IdentityFailureV1::Conflict
        );
        let session_id = session_id(request.operation_id)?;
        let operation = request.operation_id.to_string();
        anyhow::ensure!(
            !named_operation(&tx, request.operation_id)?
                && event_by_id(&tx, &session_fence_id(session_id))?.is_none(),
            IdentityFailureV1::Conflict
        );
        let session = session_id.to_string();
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_mutations WHERE operation_id=?1)
             OR EXISTS(SELECT 1 FROM workers WHERE session_id=?2)
             OR EXISTS(SELECT 1 FROM kubernetes_creates WHERE session_id=?2)",
            params![operation, session],
            |row| row.get(0),
        )?;
        anyhow::ensure!(!occupied, IdentityFailureV1::Conflict);
        let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if matches!(schema, 11..=13) {
            let reserved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM kubernetes_session_reservations WHERE session_id=?1)",
                [&session],
                |row| row.get(0),
            )?;
            anyhow::ensure!(!reserved, IdentityFailureV1::Conflict);
        }
        let record = Confirmed {
            version: 2,
            request,
            preparation_id,
            preparation_audit_hash: stored.hash,
            session_id,
            admission_authority: Some(ConfirmationAuthority {
                expires_at: expires,
                authorization_revision: initiator.authorization_revision,
            }),
        };
        let confirmed =
            self.append_legacy_session_record(&tx, &clock, CONFIRMED, id.clone(), &record)?;
        for kind in [FenceKind::Operation, FenceKind::Session] {
            self.append_legacy_session_record(
                &tx,
                &clock,
                FENCED,
                fence_id(kind, &record),
                &Fence {
                    version: 1,
                    binding_ref: id.clone(),
                    kind,
                },
            )?;
        }
        anyhow::ensure!(expires > Utc::now(), IdentityFailureV1::InvalidRequest);
        self.commit_authorized(tx, &clock)?;
        Ok(confirmed_receipt(&confirmed, &record, false))
    }
}
