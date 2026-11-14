//! Owned broker-supervision Store entries for the proposed
//! `broker_supervision_owned_v2` profile, whose wire contract is
//! `crate::broker_registry::owned_supervision`. Proposal submitted to Root:
//! nothing here opens a route, admits a credential or activates a writer.
//!
//! Every entry runs under the caller's real `HumanSourceAccess` on one checked
//! scoped transaction. The engaged identity is rebuilt inside that transaction
//! from the Store's own retained source, `tenant_binding`, `control_meta` and
//! the exact `user_version`, which must equal the retained descriptor format
//! (14 or 15). The legacy unscoped lanes (6–13, checkpoints 12–13) are not
//! extended and no format grants a permission. Presence operations and the
//! maintenance grant-receipt read use the `Business` class; grant and
//! revocation use `IdentityAuthority` under a write epoch. A heartbeat stays a
//! declaration whose reception is observed:
//! nothing here releases a quota, fences a worker, meters usage, proves a
//! runtime inventory or sends a command.
//!
//! An online owned job runs in exactly one checked transaction: the startup
//! discovery generation is compared with the source's current one, the same
//! bearer is authenticated again and must still prove the gate's broker
//! proof, the target work runs, and the generation is compared again before
//! the commit. A stale generation therefore refuses without any mutation, and
//! the generation is never compared outside this transaction. A coverage
//! closure latched elsewhere (another context's rebuild requirement) can still
//! withhold an outcome committed here; the publisher then reconciles the same
//! operation by exact replay or self-read after the operator's restart.
use super::*;
use crate::{
    audit::{
        HumanDiscoveryGeneration,
        human_modern::discovery_generation,
        review_authority::ReviewAuthorityFailure,
        workload::{
            AuthorizedControlTimeV1, capture_control_time, revalidate_broker_identity_checked,
        },
    },
    broker_registry::owned_supervision::{
        OwnedBrokerHeartbeatRequestV2, OwnedBrokerHeartbeatResultV2, OwnedBrokerIdentityViewV2,
        OwnedBrokerSelfViewV2, OwnedBrokerSubjectV2, OwnedChecked, OwnedSourceFormatV2,
        OwnedSupervisionErrorCodeV2 as Code,
    },
    human_authority::{
        HumanAuthorityFailure, HumanSourceKindV1, HumanSourcesWriteEpoch, OwnedSourceToken,
    },
    workload::{AuthenticatedWorkload, WorkloadFailureV1},
};
use std::cell::Cell;

/// Discovery-generation window of one online owned broker job: the coverage
/// generation recorded for the retained source with the complete startup
/// candidate index, and the presented bearer. Both are checked inside the
/// target transaction. Neither serializable, clonable nor a capability; the
/// offline maintenance has no window (the index is rebuilt at restart).
pub(crate) struct OwnedBrokerWindow<'a> {
    expected: HumanDiscoveryGeneration,
    token: &'a str,
    stale: Cell<bool>,
}

impl<'a> OwnedBrokerWindow<'a> {
    pub(crate) fn new(expected: HumanDiscoveryGeneration, token: &'a str) -> Self {
        Self {
            expected,
            token,
            stale: Cell::new(false),
        }
    }

    /// A transaction of this window observed another discovery generation
    /// and refused without mutation: the caller latches its rebuild
    /// requirement, which closes the human contexts like a stale read.
    pub(crate) fn stale(&self) -> bool {
        self.stale.get()
    }
}

/// How the broker proof of one owned transaction is established.
pub(super) enum OwnedProof<'a> {
    /// Offline maintenance: the entry's proof was authenticated in this
    /// retained source by the maintenance's own read; no discovery window
    /// exists offline.
    Offline,
    /// Online job: the gate's proof, which the same bearer must still prove
    /// in the target transaction, inside its discovery window.
    Windowed(&'a OwnedBrokerWindow<'a>, &'a BrokerIdentityV1),
}

/// The same broker proof: tenant, broker, credential, binding revision and
/// grant revision (the fields of the registry identity evidence). A renewed
/// binding or a new grant is another proof.
fn same_broker(first: &BrokerIdentityV1, second: &BrokerIdentityV1) -> bool {
    first.tenant_id() == second.tenant_id()
        && first.broker_id() == second.broker_id()
        && first.credential_id() == second.credential_id()
        && first.binding_revision() == second.binding_revision()
        && first.grant_revision() == second.grant_revision()
}

/// Facts of the retained tenant source engaged by the owned profile. They are
/// built only from the real retained `OwnedSourceToken` (owner, descriptor and
/// closure), never from request data, and are neither serializable nor a
/// capability. Each Store entry re-derives them from the Store's own retained
/// source inside its checked transaction and refuses any difference.
pub(crate) struct OwnedRetainedSourceFacts {
    installation_id: uuid::Uuid,
    closure_id: uuid::Uuid,
    closure_revision: u64,
    source_id: uuid::Uuid,
    namespace_id: uuid::Uuid,
    tenant_id: String,
    format: OwnedSourceFormatV2,
}

impl OwnedRetainedSourceFacts {
    /// Only a tenant Store source whose retained descriptor requires exactly
    /// format 14 or 15 participates. The descriptor is an expectation; the
    /// checked transaction still verifies the physical source and marker.
    pub(crate) fn from_token(token: &OwnedSourceToken) -> OwnedChecked<Self> {
        let descriptor = token.descriptor();
        let closure = token.closure();
        let tenant_id = match (descriptor.kind, &descriptor.tenant_id) {
            (HumanSourceKindV1::TenantStore, Some(tenant_id)) => tenant_id.clone(),
            _ => return Err(Code::SourceRebuildRequired),
        };
        let format = OwnedSourceFormatV2::from_verified(descriptor.required_format)
            .ok_or(Code::MigrationRequired)?;
        owned_check(
            descriptor.source_id == token.source_id()
                && descriptor.installation_id == closure.installation_id
                && closure.closure_revision > 0
                && revision(closure.closure_revision).is_ok(),
            Code::SourceRebuildRequired,
        )?;
        Ok(Self {
            installation_id: closure.installation_id,
            closure_id: closure.closure_id,
            closure_revision: closure.closure_revision,
            source_id: token.source_id(),
            namespace_id: descriptor.durable_namespace_id,
            tenant_id,
            format,
        })
    }

    /// Tenant named by the retained descriptor; selection evidence for the
    /// caller's retained tenant state, never a permission.
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Descriptor installation. With the accessors below, these are the pins
    /// that an offline maintenance plan records and later compares; reading
    /// them grants nothing and reconstructs no facts value.
    pub(crate) fn installation_id(&self) -> uuid::Uuid {
        self.installation_id
    }

    pub(crate) fn closure_id(&self) -> uuid::Uuid {
        self.closure_id
    }

    /// Observed closure revision of the retained closure.
    pub(crate) fn closure_revision(&self) -> u64 {
        self.closure_revision
    }

    pub(crate) fn source_id(&self) -> uuid::Uuid {
        self.source_id
    }

    /// Durable control namespace named by the retained descriptor.
    pub(crate) fn namespace_id(&self) -> uuid::Uuid {
        self.namespace_id
    }

    /// Owned format required by the retained descriptor (14 or 15).
    pub(crate) fn format(&self) -> OwnedSourceFormatV2 {
        self.format
    }

    fn same(&self, other: &Self) -> bool {
        self.installation_id == other.installation_id
            && self.closure_id == other.closure_id
            && self.closure_revision == other.closure_revision
            && self.source_id == other.source_id
            && self.namespace_id == other.namespace_id
            && self.tenant_id == other.tenant_id
            && self.format == other.format
    }
}

/// Server observation of the retained source on one checked transaction.
pub(in crate::audit) struct OwnedSourceAnchor {
    format: OwnedSourceFormatV2,
    namespace: String,
}

/// Engaged identity, broker projection and control time of one transaction.
pub(super) struct OwnedCurrent {
    pub(super) identity: OwnedBrokerIdentityViewV2,
    pub(super) broker: BrokerV1,
    pub(super) evaluated_at: String,
}

pub(super) fn owned_check(ok: bool, code: Code) -> OwnedChecked<()> {
    if ok { Ok(()) } else { Err(code) }
}

/// Closed, secret-free classification of a Store refusal. Unknown failures
/// are a temporary unavailability while the Store is healthy and an audit
/// unavailability once its integrity latch is set; never a success.
pub(super) fn classify(store: &Store, error: &anyhow::Error) -> Code {
    if let Some(failure) = error.downcast_ref::<BrokerRegistryFailureV1>() {
        return match failure {
            BrokerRegistryFailureV1::InvalidRequest => Code::InvalidRequest,
            BrokerRegistryFailureV1::Conflict => Code::SequenceConflict,
            BrokerRegistryFailureV1::NotFound | BrokerRegistryFailureV1::PermissionDenied => {
                Code::Forbidden
            }
            BrokerRegistryFailureV1::MigrationRequired => Code::MigrationRequired,
            BrokerRegistryFailureV1::ClockRegression => Code::ClockUncertain,
        };
    }
    if let Some(failure) = error.downcast_ref::<WorkloadFailureV1>() {
        return match failure {
            WorkloadFailureV1::InvalidRequest => Code::InvalidRequest,
            WorkloadFailureV1::Conflict => Code::SequenceConflict,
            WorkloadFailureV1::Unauthorized => Code::Forbidden,
            WorkloadFailureV1::MigrationRequired => Code::MigrationRequired,
            WorkloadFailureV1::ClockRegression => Code::ClockUncertain,
        };
    }
    if error.downcast_ref::<AuditCapacityFailureV1>().is_some() {
        return Code::AuditUnavailable;
    }
    if let Some(failure) = error.downcast_ref::<HumanAuthorityFailure>() {
        return match failure {
            HumanAuthorityFailure::InvalidManifest
            | HumanAuthorityFailure::ConfigurationMismatch
            | HumanAuthorityFailure::StorageBindingMismatch
            | HumanAuthorityFailure::OwnerInvalid
            | HumanAuthorityFailure::SourceMismatch => Code::SourceRebuildRequired,
            HumanAuthorityFailure::StorageUnavailable
            | HumanAuthorityFailure::OwnerContended
            | HumanAuthorityFailure::Closing
            | HumanAuthorityFailure::GateBusy
            | HumanAuthorityFailure::EpochExpired
            | HumanAuthorityFailure::TaskFailed
            | HumanAuthorityFailure::OpeningTimedOut => Code::SupervisionUnavailable,
        };
    }
    if error.downcast_ref::<ReviewAuthorityFailure>() == Some(&ReviewAuthorityFailure::Mismatch) {
        return Code::SourceRebuildRequired;
    }
    if !store.healthy() {
        return Code::AuditUnavailable;
    }
    Code::SupervisionUnavailable
}

/// Distinguish an operation identity already recorded with other content
/// (`OperationConflict`) from a sequence/revision refusal of the shared body.
/// The shared replay repeats the same comparison in the same transaction.
pub(super) fn operation_precheck(
    db: &Connection,
    change: &Change,
    input_hash: &str,
) -> OwnedChecked<()> {
    let recorded = db
        .query_row(
            "SELECT o.input_hash,e.kind FROM broker_registry_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
            [change.operation_id()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|_| Code::SupervisionUnavailable)?;
    match recorded {
        Some((hash, kind)) if hash != input_hash || kind != change.kind() => {
            Err(Code::OperationConflict)
        }
        Some(_) | None => Ok(()),
    }
}

pub(super) const HEARTBEAT_KIND: &str = "control.broker_registry.heartbeat";
const GRANT_KIND: &str = "control.broker_registry.grant";

/// Receipt of the latest registry operation of `kind` for this broker, read
/// in the caller's transaction, with its record, input hash, operation and
/// broker rechecked. For heartbeats: only heartbeats advance
/// `heartbeat_revision`, so this is the operation that produced the current
/// revision. For grants: the operation that produced the latest grant. `None`
/// means that no such operation was ever committed for this broker in this
/// tenant Store.
pub(super) fn latest_receipt(
    db: &Connection,
    broker_id: &str,
    kind: &'static str,
) -> anyhow::Result<Option<BrokerMutationReceiptV1>> {
    let row = db
        .query_row(
            "SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload,o.operation_id,o.input_hash,o.result_json FROM broker_registry_operations o JOIN events e ON e.seq=o.event_seq WHERE o.broker_id=?1 AND e.kind=?2 ORDER BY o.event_seq DESC LIMIT 1",
            params![broker_id, kind],
            |row| {
                Ok((
                    row_event(row)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((stored, operation_id, input_hash, json)) = row else {
        return Ok(None);
    };
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    require(
        record.change.kind() == kind
            && record.version == record.change.record_version()
            && record.input_hash == input_hash
            && record.input_hash == record.change.input_hash()?
            && record.change.operation_id() == operation_id
            && record.change.broker_id() == broker_id,
    )?;
    record.change.validate()?;
    let broker: BrokerV1 = serde_json::from_str(&json)?;
    require(broker.broker_id == broker_id)?;
    Ok(Some(result(&stored, broker, input_hash, operation_id, false).receipt))
}

impl Store {
    /// Exact retained-source guard of every owned entry, before any read,
    /// replay or mutation: the caller's facts equal the Store's retained
    /// source, the format equals its descriptor (14 or 15), and the tenant
    /// binding and control namespace equal the retained descriptor.
    pub(in crate::audit) fn owned_source(
        &self,
        tx: &CheckedTransaction<'_>,
        facts: &OwnedRetainedSourceFacts,
    ) -> OwnedChecked<OwnedSourceAnchor> {
        tx.require_business_access()
            .map_err(|error| classify(self, &error))?;
        let retained = self
            .review_authority
            .as_ref()
            .map(|authority| authority.source_token())
            .ok_or(Code::SourceRebuildRequired)?;
        owned_check(
            facts.same(&OwnedRetainedSourceFacts::from_token(retained)?)
                && facts.tenant_id == self.tenant_id,
            Code::SourceRebuildRequired,
        )?;
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| Code::SupervisionUnavailable)?;
        let format = u32::try_from(version)
            .ok()
            .and_then(OwnedSourceFormatV2::from_verified)
            .ok_or(Code::MigrationRequired)?;
        owned_check(format == facts.format, Code::MigrationRequired)?;
        let (tenant, binding_version): (String, i64) = tx
            .query_row(
                "SELECT tenant_id,schema_version FROM tenant_binding WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| Code::SupervisionUnavailable)?;
        owned_check(
            tenant == self.tenant_id && binding_version == 1,
            Code::SourceRebuildRequired,
        )?;
        let namespace: String = tx
            .query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| Code::SupervisionUnavailable)?;
        owned_check(
            registry_uuid(&namespace).is_ok()
                && uuid::Uuid::parse_str(&namespace).is_ok_and(|id| id == facts.namespace_id),
            Code::SourceRebuildRequired,
        )?;
        Ok(OwnedSourceAnchor { format, namespace })
    }

    /// One `Business` transaction under the caller's real source access, with
    /// the control clock captured inside it. With a window, the discovery
    /// generation is compared before the work and again before the commit,
    /// in this same transaction. A refusal before commit returns its
    /// classified code without mutation. For a mutating entry, any commit
    /// failure is `ResponseWithheld`: the outcome may be durable, so the
    /// publisher reconciles the same operation by an exact self-read.
    fn owned_transaction<T>(
        &self,
        access: &HumanSourceAccess<'_>,
        window: Option<&OwnedBrokerWindow<'_>>,
        mutating: bool,
        work: impl FnOnce(&CheckedTransaction<'_>, &AuthorizedControlTimeV1) -> OwnedChecked<T>,
    ) -> OwnedChecked<T> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| Code::SupervisionUnavailable)?;
        let tx = self
            .begin_checked_scoped(&mut db, access, StoreOperationClass::Business)
            .map_err(|error| classify(self, &error))?;
        let clock = capture_control_time(&tx).map_err(|error| classify(self, &error))?;
        if let Some(window) = window {
            self.owned_window(&tx, window)?;
        }
        let value = work(&tx, &clock)?;
        if let Some(window) = window {
            self.owned_window(&tx, window)?;
        }
        self.commit_authorized(tx, &clock).map_err(|error| {
            if mutating {
                Code::ResponseWithheld
            } else {
                classify(self, &error)
            }
        })?;
        Ok(value)
    }

    /// The owned transaction of a broker entry. An online proof is first
    /// re-authenticated from the same bearer in this transaction and must
    /// still be the gate's proof; an offline proof was authenticated by the
    /// maintenance read. Every entry then rebuilds the engaged identity.
    pub(super) fn owned_business<T>(
        &self,
        access: &HumanSourceAccess<'_>,
        proof: &OwnedProof<'_>,
        mutating: bool,
        work: impl FnOnce(&CheckedTransaction<'_>, &AuthorizedControlTimeV1) -> OwnedChecked<T>,
    ) -> OwnedChecked<T> {
        let window = match proof {
            OwnedProof::Offline => None,
            OwnedProof::Windowed(window, _) => Some(*window),
        };
        self.owned_transaction(access, window, mutating, |tx, clock| {
            if let OwnedProof::Windowed(window, claimed) = proof {
                let current = self.owned_bearer(tx, clock, window.token)?;
                owned_check(same_broker(&current, claimed), Code::Forbidden)?;
            }
            work(tx, clock)
        })
    }

    /// One discovery-generation observation inside the caller's transaction.
    /// A discordant generation marks the window stale and refuses before any
    /// mutation or commit; the caller latches the rebuild requirement.
    fn owned_window(
        &self,
        tx: &CheckedTransaction<'_>,
        window: &OwnedBrokerWindow<'_>,
    ) -> OwnedChecked<()> {
        let current = discovery_generation(tx).map_err(|error| classify(self, &error))?;
        if current != window.expected {
            window.stale.set(true);
            return Err(Code::SourceRebuildRequired);
        }
        Ok(())
    }

    /// The presented bearer authenticated inside the caller's transaction by
    /// the shared scoped workload body, then restricted to a broker of this
    /// tenant.
    fn owned_bearer(
        &self,
        tx: &CheckedTransaction<'_>,
        clock: &AuthorizedControlTimeV1,
        token: &str,
    ) -> OwnedChecked<BrokerIdentityV1> {
        let authenticated = self
            .owned_authenticate_workload_checked(tx, clock, token)
            .map_err(|error| classify(self, &error))?;
        self.owned_broker_only(authenticated)
    }

    /// Only a broker of this tenant authenticates: a worker bearer is
    /// forbidden here and an unbound digest still requires authentication.
    fn owned_broker_only(
        &self,
        authenticated: Option<AuthenticatedWorkload>,
    ) -> OwnedChecked<BrokerIdentityV1> {
        match authenticated {
            Some(AuthenticatedWorkload::Broker(identity))
                if identity.tenant_id() == self.tenant_id =>
            {
                Ok(identity)
            }
            Some(AuthenticatedWorkload::Broker(_) | AuthenticatedWorkload::Worker(_)) => {
                Err(Code::Forbidden)
            }
            None => Err(Code::AuthenticationRequired),
        }
    }

    /// Engaged identity built in the transaction: retained source facts,
    /// exact format, tenant binding, control namespace and the revalidated
    /// broker proof (binding, grant, incarnation, expiry and time floor).
    pub(super) fn owned_current(
        &self,
        tx: &CheckedTransaction<'_>,
        clock: &AuthorizedControlTimeV1,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedCurrent> {
        let fail = |error: anyhow::Error| classify(self, &error);
        let anchor = self.owned_source(tx, facts)?;
        owned_check(identity.tenant_id() == self.tenant_id, Code::Forbidden)?;
        let now = clock.at();
        let evaluated_at = format_time(now);
        revalidate_broker_identity_checked(tx, identity, now).map_err(fail)?;
        let broker = load_broker(tx, identity.broker_id())
            .map_err(fail)?
            .ok_or(Code::Forbidden)?;
        broker
            .require_current_grant(identity, &evaluated_at)
            .map_err(fail)?;
        let subject = OwnedBrokerSubjectV2 {
            installation_id: facts.installation_id.to_string(),
            closure_id: facts.closure_id.to_string(),
            source_id: facts.source_id.to_string(),
            source_format: anchor.format.number(),
            tenant_id: self.tenant_id.clone(),
            control_namespace_uuid: anchor.namespace,
            broker_id: broker.broker_id.clone(),
            incarnation_id: broker.incarnation_id.clone(),
            credential_id: identity.credential_id().to_owned(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        };
        let view = OwnedBrokerIdentityViewV2::new(facts.closure_revision, subject)
            .map_err(|_| Code::ResponseUnpublishable)?;
        Ok(OwnedCurrent {
            identity: view,
            broker,
            evaluated_at,
        })
    }

    /// The pure checks the legacy wrapper performs before its lock: a valid
    /// typed change whose serialization needs no redaction.
    pub(super) fn owned_change_input(&self, change: &Change) -> OwnedChecked<()> {
        change.validate().map_err(|_| Code::InvalidRequest)?;
        let value = serde_json::to_value(change).map_err(|_| Code::InvalidRequest)?;
        owned_check(self.redactor.value(&value) == value, Code::InvalidRequest)
    }

    /// Publication screen: a positive DTO that the Store dictionary would mask
    /// is refused rather than published altered. It runs before commit, so
    /// the deterministic refusal (`ResponseUnpublishable`, wait for operator)
    /// rolls the transaction back.
    pub(super) fn owned_public<T: Serialize>(&self, value: &T) -> OwnedChecked<()> {
        let value = serde_json::to_value(value).map_err(|_| Code::ResponseUnpublishable)?;
        owned_check(
            self.redactor.value_unclipped(&value) == value,
            Code::ResponseUnpublishable,
        )
    }

    /// Closed classification of a Store refusal for the owned API layer, with
    /// the exact mapping of every owned entry. It never yields a success and
    /// never carries the refused error's prose, path or value.
    pub(crate) fn owned_failure_code(&self, error: &anyhow::Error) -> Code {
        classify(self, error)
    }

    /// Offline owned broker authentication of a presented bearer in the
    /// caller's retained source, under its real `HumanSourceAccess` (used by
    /// the maintenance handoff). The scoped workload authentication
    /// revalidates the binding (active, unexpired, current revision), the
    /// current grant, the incarnation and the control time floor in one
    /// checked transaction. Only a broker of this tenant authenticates: a
    /// worker bearer is forbidden here and an unbound digest still requires
    /// authentication. The engaged identity of each operation is rebuilt
    /// later, in that operation's own transaction.
    pub(crate) fn owned_authenticate_broker(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        token: &str,
    ) -> OwnedChecked<BrokerIdentityV1> {
        owned_check(
            facts.tenant_id == self.tenant_id,
            Code::SourceRebuildRequired,
        )?;
        let authenticated = self
            .authenticate_workload_token_scoped(access, token)
            .map_err(|error| classify(self, &error))?;
        self.owned_broker_only(authenticated)
    }

    /// Online gate authentication of a presented bearer in one checked
    /// transaction of the candidate's retained source, inside its discovery
    /// window: generation, retained source (facts, exact format, tenant
    /// binding, namespace), the shared workload authentication restricted to
    /// a broker of this tenant, generation again, then a read-only commit.
    pub(crate) fn owned_window_authenticate(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
    ) -> OwnedChecked<BrokerIdentityV1> {
        self.owned_transaction(access, Some(window), false, |tx, clock| {
            self.owned_source(tx, facts)?;
            self.owned_bearer(tx, clock, window.token)
        })
    }

    /// Instantaneous engaged identity for an offline maintenance proof.
    /// Reading it refreshes no heartbeat and authorizes no later operation.
    pub(crate) fn owned_broker_identity(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedBrokerIdentityViewV2> {
        self.owned_identity_view(access, &OwnedProof::Offline, facts, identity)
    }

    /// Instantaneous engaged identity for the gate's proof, re-authenticated
    /// from the same bearer inside the discovery window of this transaction.
    pub(crate) fn owned_window_identity(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedBrokerIdentityViewV2> {
        self.owned_identity_view(
            access,
            &OwnedProof::Windowed(window, identity),
            facts,
            identity,
        )
    }

    fn owned_identity_view(
        &self,
        access: &HumanSourceAccess<'_>,
        proof: &OwnedProof<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedBrokerIdentityViewV2> {
        self.owned_business(access, proof, false, |tx, clock| {
            let current = self.owned_current(tx, clock, facts, identity)?;
            self.owned_public(&current.identity)?;
            Ok(current.identity)
        })
    }

    /// Identity, broker view and the latest heartbeat receipt, all read in the
    /// same checked transaction so that a publisher can recognize only its
    /// exact committed operation after a lost response.
    pub(crate) fn owned_broker_self_view(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
    ) -> OwnedChecked<OwnedBrokerSelfViewV2> {
        let proof = OwnedProof::Windowed(window, identity);
        self.owned_business(access, &proof, false, |tx, clock| {
            let fail = |error: anyhow::Error| classify(self, &error);
            let current = self.owned_current(tx, clock, facts, identity)?;
            let receipt =
                latest_receipt(tx, &current.broker.broker_id, HEARTBEAT_KIND).map_err(fail)?;
            let view = current
                .broker
                .view_at(&current.evaluated_at)
                .map_err(fail)?;
            let value = OwnedBrokerSelfViewV2::new(current.identity, view, receipt)
                .map_err(|_| Code::ResponseUnpublishable)?;
            self.owned_public(&value)?;
            Ok(value)
        })
    }

    /// Owned heartbeat. Inside one `Business` transaction: the engaged
    /// identity is rebuilt, the request subject and both commitments are
    /// compared with it, the private registry `Change` hash is recomputed and
    /// must equal the shared mirror, all before any replay or mutation. The
    /// shared registry body then revalidates the proof, returns the original
    /// receipt for an exact replay or appends the audit event and projection.
    /// The positive result is built and screened before commit.
    pub(crate) fn owned_broker_heartbeat(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        identity: &BrokerIdentityV1,
        request: &OwnedBrokerHeartbeatRequestV2,
    ) -> OwnedChecked<OwnedBrokerHeartbeatResultV2> {
        request.validate()?;
        let change = Change::Heartbeat {
            identity: IdentityEvidence::capture(identity),
            heartbeat: request.heartbeat.clone(),
        };
        self.owned_change_input(&change)?;
        let proof = OwnedProof::Windowed(window, identity);
        self.owned_business(access, &proof, true, |tx, clock| {
            let current = self.owned_current(tx, clock, facts, identity)?;
            let commitments = request.verify_for(&current.identity)?;
            let private = change.input_hash().map_err(|_| Code::InvalidRequest)?;
            owned_check(
                private == commitments.registry_input_hash,
                Code::ProtocolMismatch,
            )?;
            operation_precheck(tx, &change, &private)?;
            let mutation = self
                .persist_broker_change_checked(tx, clock, change, Some(identity))
                .map_err(|error| classify(self, &error))?;
            let value = OwnedBrokerHeartbeatResultV2::new(
                current.identity,
                commitments.operation_commitment,
                mutation,
            )
            .map_err(|_| Code::ResponseUnpublishable)?;
            self.owned_public(&value)?;
            Ok(value)
        })
    }

    /// Receipt of the grant operation that produced this broker's current,
    /// unrevoked grant, read under the caller's scoped access. A later
    /// revocation (or any grant revision not produced by that operation) is a
    /// conflict, never an older receipt. Used by the offline maintenance to
    /// build an enrollment handoff from committed receipts only.
    pub(crate) fn owned_broker_grant_receipt(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        broker_id: &str,
    ) -> anyhow::Result<BrokerMutationReceiptV1> {
        registry_uuid(broker_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?;
        self.owned_source(&tx, facts)?;
        require_checked_schema(&tx)?;
        let broker = load_broker(&tx, broker_id)?.ok_or(BrokerRegistryFailureV1::NotFound)?;
        let receipt =
            latest_receipt(&tx, broker_id, GRANT_KIND)?.ok_or(BrokerRegistryFailureV1::NotFound)?;
        conflict(
            !broker.revoked
                && receipt.broker_id == broker.broker_id
                && receipt.grant_revision == broker.grant_revision,
        )?;
        self.commit_checked(tx)?;
        Ok(receipt)
    }

    /// Identity-authority grant for an owned broker, under a real write epoch
    /// of the retained source. The owned profile v1 pins exactly one authority
    /// per grant, never of the Kubernetes runtime kind (no Kubernetes broker
    /// adapter is admitted); the registry still requires a fresh incarnation
    /// per grant. Offline maintenance is the recommended caller (decision D1).
    pub(crate) fn owned_broker_grant(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        mutation: &BrokerGrantMutationV1,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        mutation.validate()?;
        require(matches!(
            mutation.authorities.as_slice(),
            [only] if only.runtime_kind != BrokerRuntimeKindV1::Kubernetes
        ))?;
        self.owned_authority_change(
            epoch,
            source,
            Change::Grant {
                mutation: mutation.clone(),
            },
        )
    }

    /// Identity-authority revocation of an owned broker grant. History is kept;
    /// an extracted proof of the revoked grant fails its next revalidation.
    pub(crate) fn owned_broker_revoke(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        mutation: &BrokerRevocationV1,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        mutation.validate()?;
        self.owned_authority_change(
            epoch,
            source,
            Change::Revoke {
                mutation: mutation.clone(),
            },
        )
    }

    fn owned_authority_change(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        change: Change,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        let facts = OwnedRetainedSourceFacts::from_token(source)?;
        change.validate()?;
        let value = serde_json::to_value(&change)?;
        require(self.redactor.value(&value) == value)?;
        let access = HumanSourceAccess::write(epoch, source)?;
        // Never wait on the control mutex while retaining a write epoch.
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        // Class and epoch are proven before the shared replay/idempotence.
        tx.require_identity_authority()?;
        self.owned_source(&tx, &facts)?;
        require_checked_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        let result = self.persist_broker_change_checked(&tx, &clock, change, None)?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }
}
