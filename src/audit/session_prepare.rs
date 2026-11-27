//! Explicit nominative launch-preparation capability (`session_prepare_v1`),
//! installed in place on adopted owned sources (Store14 and Store15) by the
//! journaled offline identity-marker maintenance. The marker is an audit-chain
//! event, not a store format number: the preparation tables exist exactly when
//! the validated genesis does, so a reader that predates the capability refuses
//! the extended source at open. Sources without the marker keep their current
//! contract; the capability grants no permission, context, read or mutation.
//!
//! A retained preparation links the operation, the authenticated initiator and
//! its context to the mandatory audit receipt and the target's server revision.
//! Prepare persists only that receipt: it creates no session grant, reservation
//! or start claim. The launch later engages the exact intent and creates the
//! nominative grant through the existing identity grant producer.
use super::*;
use crate::control::session_control::{
    ProvisionSessionLaunchTargetV1, SessionConversationBindingV1, SessionConversationStatusV1,
    SessionLaunchPrepareRequestV1, SessionLaunchReceiptV1, SessionLaunchRequestV1,
    SessionLaunchTargetRecordV1, SessionLaunchTargetSelectionV1, SessionPrepareReceiptV1,
};
use crate::identity::{
    AuthorizedScopeV1, ContextModeV1, HumanCapabilityV1, IdentityChangeV1, IdentityFailureV1,
    IdentityMutationV1, LegacyControlRoleV1, PrincipalRefV1, ResourceGrantV1, ResourceKindV1,
    ResourceRefV1,
};
use uuid::Uuid;

pub(super) mod catalog;

pub(crate) const GENESIS_KIND: &str = "control.session_prepare.genesis";
pub(crate) const PREPARED_KIND: &str = "control.session_prepare.prepared";
pub(crate) const LAUNCHED_KIND: &str = "control.session_prepare.launched";
const TARGET_PROVISION_KIND: &str = "control.session_prepare.target_provisioned";
const PROTOCOL: &str = "session_prepare_v1";
const MARKER_VERSION: u32 = 1;
const LAUNCH_EVENT_DOMAIN: &[u8] = b"ai-guard:session-launch:v1";
const LAUNCH_SESSION_DOMAIN: &[u8] = b"ai-guard:session-launch-session:v1";
const LAUNCH_GRANT_DOMAIN: &[u8] = b"ai-guard:session-launch-grant:v1";
const LAUNCH_GRANT_ACTOR: &str = "session-launch-control";

const TARGETS_TABLE: &str = "session_launch_targets";
const TARGET_OPERATIONS_TABLE: &str = "session_launch_target_operations";
const PREPARATIONS_TABLE: &str = "session_preparations";
const LAUNCHES_TABLE: &str = "session_launches";

const TARGETS_SCHEMA: &str = "CREATE TABLE session_launch_targets(launch_target_id TEXT PRIMARY KEY,revision INTEGER NOT NULL,tenant_id TEXT NOT NULL,organization_scope TEXT NOT NULL,active INTEGER NOT NULL CHECK(active IN (0,1)),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))";
const TARGET_OPERATIONS_SCHEMA: &str = "CREATE TABLE session_launch_target_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))";
const PREPARATIONS_SCHEMA: &str = "CREATE TABLE session_preparations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,principal_key TEXT NOT NULL,membership_id TEXT NOT NULL,organization_scope TEXT NOT NULL,launch_target_id TEXT NOT NULL,target_revision INTEGER NOT NULL,expires_at TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL)";
const LAUNCHES_SCHEMA: &str = "CREATE TABLE session_launches(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,session_id TEXT NOT NULL,preparation_operation_id TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL,receipt_json TEXT NOT NULL)";

pub(crate) const SCHEMA: [(&str, &str, &str); 4] = [
    ("table", TARGETS_TABLE, TARGETS_SCHEMA),
    ("table", TARGET_OPERATIONS_TABLE, TARGET_OPERATIONS_SCHEMA),
    ("table", PREPARATIONS_TABLE, PREPARATIONS_SCHEMA),
    ("table", LAUNCHES_TABLE, LAUNCHES_SCHEMA),
];
pub(crate) const AUTO_INDEX_COUNTS: [(&str, u32); 4] = [
    (TARGETS_TABLE, 1),
    (TARGET_OPERATIONS_TABLE, 1),
    (PREPARATIONS_TABLE, 1),
    (LAUNCHES_TABLE, 1),
];

/// The capability exists only on the owned formats it is installed on.
pub(crate) fn format_admits(version: i64) -> bool {
    matches!(version, 14 | 15)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    control_namespace_uuid: String,
    human_rights_seq: i64,
    authorization_revision: u64,
    previous_seq: i64,
    previous_hash: String,
}

/// The marker exists in the chain (schema admission only, not a validation).
pub(crate) fn capability_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
        [GENESIS_KIND],
        |row| row.get(0),
    )?)
}

fn table_present(db: &Connection, table: &str) -> anyhow::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

/// The validated genesis, or `None` when the chain does not carry it. More than
/// one marker, or one that differs from its durable prefix, is invalid.
fn genesis(db: &Connection) -> anyhow::Result<Option<StoredEvent>> {
    let records = {
        let mut statement = db.prepare(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2",
        )?;
        statement
            .query_map([GENESIS_KIND], row_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    anyhow::ensure!(records.len() <= 1, "invalid session prepare marker count");
    let Some(stored) = records.into_iter().next() else {
        return Ok(None);
    };
    let marker: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let rights: Option<i64> = db.query_row(
        "SELECT MAX(seq) FROM events WHERE kind=?1",
        [super::human_rights::GENESIS_KIND],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        marker.version == MARKER_VERSION
            && marker.protocol == PROTOCOL
            && marker.control_namespace_uuid == namespace
            && rights == Some(marker.human_rights_seq)
            && stored.seq > marker.human_rights_seq
            && marker.previous_seq == stored.seq - 1
            && marker.previous_hash == stored.prev_hash
            && i64::try_from(marker.authorization_revision).is_ok()
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native",
        "invalid session prepare marker"
    );
    Ok(Some(stored))
}

/// Whether this owned source carries the validated preparation capability.
pub(crate) fn installed(db: &Connection) -> anyhow::Result<bool> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(format_admits(version) && genesis(db)?.is_some())
}

/// Schema validation: the tables exist exactly when the validated marker does,
/// and only on an owned format. A table without its marker, or a marker without
/// its tables, fails closed.
pub(super) fn validate_capability(
    db: &Connection,
    version: i64,
    historical: Option<catalog::HistoricalCatalog>,
) -> anyhow::Result<()> {
    let present = capability_present(db)?;
    let tables = table_present(db, TARGETS_TABLE)?
        && table_present(db, TARGET_OPERATIONS_TABLE)?
        && table_present(db, PREPARATIONS_TABLE)?;
    let launches = table_present(db, LAUNCHES_TABLE)?;
    let message_tables = super::session_message::tables_present(db)?;
    if present || tables || launches || message_tables {
        anyhow::ensure!(
            format_admits(version)
                && present
                && tables
                && launches == catalog::includes_prepare_table(historical, LAUNCHES_TABLE)
                && message_tables == historical.is_none(),
            "session prepare capability requires its marker and tables on Store14/15"
        );
        let original = genesis(db)?.context("session prepare genesis missing")?;
        if let Some(historical) = historical {
            catalog::validate_historical_audit(db, historical, None)?;
            catalog::validate_historical_projections(db, historical, original.seq)?;
        }
    }
    Ok(())
}

/// The closed payload of one retained preparation event.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    version: u32,
    operation_id: String,
    principal_key: String,
    membership_id: String,
    organization_scope: String,
    launch_target_id: String,
    target_revision: u64,
    input_hash: String,
    expires_at: String,
}

/// The closed payload of one committed launch event. It carries the session id
/// (the launch creates the session identity), the consumed preparation reference
/// and exact fingerprint, the nominative grant operation and the retained
/// conversation binding.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launched {
    version: u32,
    operation_id: String,
    principal_key: String,
    membership_id: String,
    organization_scope: String,
    launch_target_id: String,
    target_revision: u64,
    preparation_operation_id: String,
    preparation_input_hash: String,
    session_id: String,
    input_hash: String,
    grant_operation_id: String,
    conversation: SessionConversationBindingV1,
}

/// Replay validation of one `control.session_prepare.*` event inside the
/// closed control replay. The marker revalidates against its durable prefix;
/// the preparation and catalog payloads are closed records. The preparation and
/// catalog events carry no session id; the launch event carries the session it
/// creates and binds it to its own identifier.
pub(crate) fn validate_replay_event(db: &Connection, stored: &StoredEvent) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native",
        "unexpected session prepare event provenance"
    );
    if stored.event.kind == LAUNCHED_KIND {
        let record: Launched = serde_json::from_value(stored.event.data.clone())?;
        anyhow::ensure!(
            record.version == 1
                && crate::control::session_control::canonical_uuid(&record.operation_id).is_ok()
                && crate::control::session_control::canonical_uuid(&record.launch_target_id)
                    .is_ok()
                && crate::control::session_control::canonical_uuid(
                    &record.preparation_operation_id
                )
                .is_ok()
                && crate::control::session_control::canonical_uuid(&record.session_id).is_ok()
                && crate::control::session_control::canonical_uuid(&record.grant_operation_id)
                    .is_ok()
                && (1..=i64::MAX as u64).contains(&record.target_revision)
                && !record.principal_key.is_empty()
                && !record.membership_id.is_empty()
                && !record.organization_scope.is_empty()
                && !record.input_hash.is_empty()
                && !record.preparation_input_hash.is_empty()
                && record.conversation.version == 1
                && record.conversation.session_id == record.session_id
                && record.conversation.status == SessionConversationStatusV1::Prepared
                && chrono::DateTime::parse_from_rfc3339(&record.conversation.expires_at).is_ok()
                && stored.event.session_id.as_deref() == Some(record.session_id.as_str()),
            "invalid session launch record"
        );
        return Ok(());
    }
    anyhow::ensure!(
        stored.event.session_id.is_none(),
        "unexpected session prepare event provenance"
    );
    match stored.event.kind.as_str() {
        GENESIS_KIND => {
            anyhow::ensure!(genesis(db)?.is_some(), "invalid session prepare marker");
        }
        catalog::UPGRADED_KIND => {
            anyhow::ensure!(
                catalog::upgraded_from(db)?.is_some(),
                "invalid session catalog upgrade"
            );
        }
        PREPARED_KIND => {
            let record: Prepared = serde_json::from_value(stored.event.data.clone())?;
            anyhow::ensure!(
                record.version == 1
                    && crate::control::session_control::canonical_uuid(&record.operation_id)
                        .is_ok()
                    && crate::control::session_control::canonical_uuid(&record.launch_target_id)
                        .is_ok()
                    && (1..=i64::MAX as u64).contains(&record.target_revision)
                    && !record.principal_key.is_empty()
                    && !record.membership_id.is_empty()
                    && !record.organization_scope.is_empty()
                    && !record.input_hash.is_empty()
                    && chrono::DateTime::parse_from_rfc3339(&record.expires_at).is_ok(),
                "invalid session preparation record"
            );
        }
        TARGET_PROVISION_KIND => {
            let command: ProvisionSessionLaunchTargetV1 =
                serde_json::from_value(stored.event.data.clone())?;
            command.validate()?;
        }
        _ => anyhow::bail!("unknown session prepare event kind"),
    }
    Ok(())
}

/// The marker event, bound to its durable prefix and the human-rights marker it
/// follows. Refuses a second marker or a missing rights marker. The caller
/// chooses the event id and the appender.
pub(crate) fn marker_event(tx: &Transaction<'_>) -> anyhow::Result<Event> {
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    anyhow::ensure!(
        format_admits(version),
        "session prepare capability requires an owned Store14/15 source"
    );
    anyhow::ensure!(
        genesis(tx)?.is_none(),
        "session prepare marker already installed"
    );
    anyhow::ensure!(
        super::human_rights::installed(tx)?,
        "session prepare capability requires the human rights marker"
    );
    let human_rights_seq: i64 = tx.query_row(
        "SELECT MAX(seq) FROM events WHERE kind=?1",
        [super::human_rights::GENESIS_KIND],
        |r| r.get(0),
    )?;
    let control_namespace_uuid = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let mut event = Event::new(GENESIS_KIND);
    event.coverage = "native".into();
    event.data = serde_json::to_value(Genesis {
        version: MARKER_VERSION,
        protocol: PROTOCOL.into(),
        control_namespace_uuid,
        human_rights_seq,
        authorization_revision: identity::authorization_revision(tx)?,
        previous_seq,
        previous_hash,
    })?;
    Ok(event)
}

/// Create the capability tables inside the held maintenance transaction, before
/// the genesis event is appended. Idempotent within the install: a second call
/// on an already-capable source is refused by `marker_event`.
pub(crate) fn install_schema(tx: &Transaction<'_>) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    // The conversation/message tables share this capability marker.
    super::session_message::install_schema(tx)?;
    Ok(())
}

fn failure(error: IdentityFailureV1) -> anyhow::Error {
    error.into()
}

/// Revalidate the caller's personal launch context inside the transaction. A
/// revoked identity, membership or authorization revision closes the effect.
fn authorize_prepare(
    tx: &CheckedTransaction<'_>,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    if !installed(tx)? {
        return Err(failure(IdentityFailureV1::MigrationRequired));
    }
    let current = identity::revalidate_scope_checked(tx, scope, tenant)?;
    if current.mode() != ContextModeV1::Personal
        || !current.principal().is_human()
        || !current.has_capability(HumanCapabilityV1::LaunchSession)
    {
        return Err(failure(IdentityFailureV1::PermissionDenied));
    }
    Ok(current)
}

/// Check the recorded initiator after revalidating the current tenant scope.
fn authorize_receipt_initiator(
    current: &AuthorizedScopeV1,
    recorded_principal_key: &str,
    recorded_membership_id: &str,
    recorded_organization_scope: &str,
) -> anyhow::Result<String> {
    let principal_key = current.principal().key()?;
    anyhow::ensure!(
        principal_key == recorded_principal_key,
        IdentityFailureV1::PermissionDenied
    );
    anyhow::ensure!(
        current.membership_id() == recorded_membership_id
            && current.organization_scope() == recorded_organization_scope,
        IdentityFailureV1::ContextRevoked
    );
    Ok(principal_key)
}

/// Resolve one active catalog target and check the exact selection against it.
/// Shared by Prepare (retains the intent) and Launch (commits it): both must
/// resolve the current server revision, so a stale expectation is a conflict.
fn authorize_target(
    tx: &CheckedTransaction<'_>,
    current: &AuthorizedScopeV1,
    launch_target_id: Uuid,
    selection: SessionLaunchTargetSelectionV1,
) -> anyhow::Result<SessionLaunchTargetRecordV1> {
    let row: Option<String> = tx
        .query_row(
            "SELECT payload_json FROM session_launch_targets WHERE launch_target_id=?1",
            [launch_target_id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    let target: SessionLaunchTargetRecordV1 =
        serde_json::from_str(&row.ok_or(failure(IdentityFailureV1::NotFound))?)?;
    target.validate()?;
    // The policy carries the target's current server revision, so the exact
    // selection check refuses a stale expectation instead of comparing the
    // request against itself.
    let policy = crate::identity::SessionLaunchTargetPolicyV1 {
        selection: SessionLaunchTargetSelectionV1::new(target.launch_target_id, target.revision)?,
        active: target.active,
        tenant_id: &target.tenant_id,
        organization_scope: &target.organization_scope,
        launchers: &target.launchers,
    };
    policy
        .authorize_launch(current, selection)
        .map_err(|error| match error {
            crate::identity::SessionLaunchTargetFailureV1::Stale => {
                failure(IdentityFailureV1::Conflict)
            }
            crate::identity::SessionLaunchTargetFailureV1::Identity(inner) => failure(inner),
        })?;
    Ok(target)
}

impl Store {
    /// Offline provisioning of one opaque launch target under the source write
    /// epoch. Exact replay returns the committed receipt; a divergent command
    /// under the same operation is a conflict. No HTTP route writes the catalog.
    // Closed session launch target catalog; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn provision_session_launch_target_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        command: &ProvisionSessionLaunchTargetV1,
    ) -> anyhow::Result<SessionPrepareReceiptV1> {
        command
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if !installed(&tx)? {
            return Err(failure(IdentityFailureV1::MigrationRequired));
        }
        let input_hash = command.input_hash()?;
        let operation_id = command.operation_id.to_string();
        if let Some(row) = target_operation(&tx, &operation_id)? {
            anyhow::ensure!(row.input_hash == input_hash, IdentityFailureV1::Conflict);
            let event = stored_event(&tx, row.event_seq)?;
            return Ok(target_receipt(command, &event, row.event_hash, true));
        }
        let existing: Option<i64> = tx
            .query_row(
                "SELECT revision FROM session_launch_targets WHERE launch_target_id=?1",
                [command.target.launch_target_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        anyhow::ensure!(
            existing.unwrap_or(0) == command.expected_revision as i64,
            IdentityFailureV1::Conflict
        );
        let clock = workload::capture_control_time(&tx)?;
        let mut event = Event::new(TARGET_PROVISION_KIND);
        event.id = provision_event_id(command.operation_id);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.data = serde_json::to_value(command)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session launch target requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        tx.execute(
            "INSERT INTO session_launch_targets(launch_target_id,revision,tenant_id,organization_scope,active,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(launch_target_id) DO UPDATE SET revision=excluded.revision,tenant_id=excluded.tenant_id,organization_scope=excluded.organization_scope,active=excluded.active,payload_json=excluded.payload_json,event_seq=excluded.event_seq",
            params![
                command.target.launch_target_id.to_string(),
                command.target.revision as i64,
                command.target.tenant_id,
                command.target.organization_scope,
                command.target.active,
                serde_json::to_string(&command.target)?,
                stored.seq,
            ],
        )?;
        tx.execute(
            "INSERT INTO session_launch_target_operations(operation_id,input_hash,event_seq) VALUES(?1,?2,?3)",
            params![operation_id, input_hash, stored.seq],
        )?;
        self.commit_authorized(tx, &clock)?;
        let hash = stored.hash.clone();
        Ok(target_receipt(command, &stored, hash, false))
    }

    /// Retain one nominative launch preparation on the owned source under its
    /// write epoch. The transaction revalidates Store authority, the personal
    /// launch context, the authorized catalog target, its revision and the
    /// deadline before the mandatory audit event and the preparation row commit
    /// together. Prepare creates no grant, reservation or claim. An exact replay
    /// returns the original receipt to the still-authorized initiator. Divergent
    /// intent under the same operation is a conflict; revocation closes the replay.
    pub(crate) fn modern_session_launch_prepare(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        scope: &AuthorizedScopeV1,
        request: &SessionLaunchPrepareRequestV1,
        expected: HumanDiscoveryGeneration,
    ) -> anyhow::Result<(SessionPrepareReceiptV1, Option<HumanDiscoveryGeneration>)> {
        request
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx = self.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        if human_modern::discovery_generation(&tx)? != expected {
            return Err(HumanDiscoveryStale.into());
        }
        // Authorization is revalidated before any replay disclosure: a revoked
        // caller never receives the retained receipt.
        let current = authorize_prepare(&tx, scope, &self.tenant_id)?;
        let clock = workload::capture_control_time(&tx)?;
        let expires_at = chrono::DateTime::parse_from_rfc3339(&request.expires_at)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?
            .with_timezone(&chrono::Utc);
        anyhow::ensure!(expires_at > clock.at(), IdentityFailureV1::InvalidRequest);
        let input_hash = request.input_hash()?;
        let operation_id = request.operation_id.to_string();
        if let Some(row) = preparation_row(&tx, &operation_id)? {
            authorize_receipt_initiator(
                &current,
                &row.principal_key,
                &row.membership_id,
                &row.organization_scope,
            )?;
            anyhow::ensure!(row.input_hash == input_hash, IdentityFailureV1::Conflict);
            let event = stored_event(&tx, row.event_seq)?;
            anyhow::ensure!(event.hash == row.event_hash, IdentityFailureV1::Conflict);
            return Ok((preparation_receipt(request, &event, true), None));
        }
        // A new preparation resolves the current catalog target; an exact replay
        // above serves the retained receipt even if the target has since advanced.
        let target = authorize_target(
            &tx,
            &current,
            request.launch_target_id,
            request.selection()?,
        )?;
        let principal_key = current.principal().key()?;
        let mut event = Event::new(PREPARED_KIND);
        event.id = preparation_event_id(request.operation_id);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        // The preparation uses its own identifier and leaves session_id empty.
        event.session_id = None;
        event.data = serde_json::json!({
            "version": 1,
            "operation_id": operation_id,
            "principal_key": principal_key,
            "membership_id": current.membership_id(),
            "organization_scope": current.organization_scope(),
            "launch_target_id": request.launch_target_id.to_string(),
            "target_revision": target.revision,
            "input_hash": input_hash,
            "expires_at": request.expires_at,
        });
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session preparation requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        tx.execute(
            "INSERT INTO session_preparations(operation_id,input_hash,principal_key,membership_id,organization_scope,launch_target_id,target_revision,expires_at,event_seq,event_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                operation_id,
                input_hash,
                principal_key,
                current.membership_id(),
                current.organization_scope(),
                request.launch_target_id.to_string(),
                target.revision as i64,
                request.expires_at,
                stored.seq,
                stored.hash,
            ],
        )?;
        // Prepare changes no identity authorization revision and no workload
        // binding: the discovery generation is unchanged, so no rebuild.
        self.commit_authorized(tx, &clock)?;
        Ok((preparation_receipt(request, &stored, false), None))
    }

    /// Commit one retained preparation as a real nominative session launch on
    /// the owned source under its write epoch. The identity-authority transaction
    /// revalidates Store authority, the personal launch context, the exact retained
    /// preparation (initiator, membership, fingerprint, deadline) and the current
    /// catalog target before any effect. It then appends the mandatory launch audit
    /// event, creates the nominative session grant through the existing identity
    /// grant producer, and retains the conversation binding — all atomically.
    /// The still-authorized initiator can replay the exact committed intent.
    /// Divergent intent under the same operation is a conflict. A source/config
    /// change or an expired preparation closes a new launch; revocation closes both.
    ///
    /// This proves the durable launch authority and grant. The supervised worker
    /// admission and the runtime-observed start are the broker's separate effect,
    /// reconciled against this binding; they are not fabricated here.
    pub(crate) fn modern_session_launch(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        scope: &AuthorizedScopeV1,
        request: &SessionLaunchRequestV1,
        expected: HumanDiscoveryGeneration,
    ) -> anyhow::Result<(SessionLaunchReceiptV1, Option<HumanDiscoveryGeneration>)> {
        request
            .validate()
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        // Launch creates a nominative grant: it is an identity-authority effect,
        // not a Business read. The same checked transaction also appends the
        // mandatory launch audit and retains the binding before commit.
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        tx.require_identity_authority()?;
        if human_modern::discovery_generation(&tx)? != expected {
            return Err(HumanDiscoveryStale.into());
        }
        // Authorization is revalidated before any replay disclosure: a revoked
        // caller never receives the retained receipt.
        let current = authorize_prepare(&tx, scope, &self.tenant_id)?;
        let clock = workload::capture_control_time(&tx)?;
        let input_hash = request.input_hash()?;
        let operation_id = request.operation_id.to_string();
        if let Some(row) = launch_row(&tx, &operation_id)? {
            let event = stored_event(&tx, row.event_seq)?;
            anyhow::ensure!(
                event.hash == row.event_hash && event.event.kind == LAUNCHED_KIND,
                IdentityFailureV1::Conflict
            );
            let launched: Launched = serde_json::from_value(event.event.data)?;
            authorize_receipt_initiator(
                &current,
                &launched.principal_key,
                &launched.membership_id,
                &launched.organization_scope,
            )?;
            anyhow::ensure!(row.input_hash == input_hash, IdentityFailureV1::Conflict);
            let mut receipt: SessionLaunchReceiptV1 = serde_json::from_str(&row.receipt_json)?;
            receipt.replayed = true;
            return Ok((receipt, None));
        }
        // Consume the exact retained preparation. Only its nominative initiator,
        // in the same membership and organization, may commit it.
        let preparation_operation_id = request.preparation_operation_id.to_string();
        let preparation = preparation_row(&tx, &preparation_operation_id)?
            .ok_or(failure(IdentityFailureV1::NotFound))?;
        let principal_key = authorize_receipt_initiator(
            &current,
            &preparation.principal_key,
            &preparation.membership_id,
            &preparation.organization_scope,
        )?;
        anyhow::ensure!(
            preparation.input_hash == request.preparation_input_hash,
            IdentityFailureV1::Conflict
        );
        anyhow::ensure!(
            preparation.launch_target_id == request.launch_target_id.to_string()
                && preparation.target_revision == request.expected_target_revision as i64,
            IdentityFailureV1::Conflict
        );
        let expires_at = chrono::DateTime::parse_from_rfc3339(&preparation.expires_at)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?
            .with_timezone(&chrono::Utc);
        anyhow::ensure!(expires_at > clock.at(), IdentityFailureV1::InvalidRequest);
        let preparation_event = stored_event(&tx, preparation.event_seq)?;
        anyhow::ensure!(
            preparation_event.hash == preparation.event_hash,
            IdentityFailureV1::Conflict
        );
        // One preparation commits at most one launch.
        anyhow::ensure!(
            consumed_by_launch(&tx, &preparation_operation_id)?.is_none(),
            IdentityFailureV1::Conflict
        );
        // Re-resolve the current catalog target: a source/config change (advanced
        // revision, deactivation or ACL change) invalidates the preparation.
        let target = authorize_target(
            &tx,
            &current,
            request.launch_target_id,
            request.selection()?,
        )?;
        anyhow::ensure!(
            target.revision == request.expected_target_revision,
            IdentityFailureV1::Conflict
        );
        // The session identity is deterministic from the launch operation, so a
        // replay reconciles to the same session without a second allocation.
        let session_id = bound_event_id(LAUNCH_SESSION_DOMAIN, request.operation_id);
        let grant_operation_id = bound_event_id(LAUNCH_GRANT_DOMAIN, request.operation_id);
        let source_id: String = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let conversation = SessionConversationBindingV1 {
            version: 1,
            tenant_id: self.tenant_id.clone(),
            source_id,
            session_id: session_id.clone(),
            launch_target_id: request.launch_target_id,
            target_revision: target.revision,
            broker_id: None,
            broker_incarnation: None,
            worker_id: None,
            attempt_id: None,
            runtime_instance: None,
            input_epoch: 0,
            provider: None,
            protocol: None,
            expires_at: preparation.expires_at.clone(),
            status: SessionConversationStatusV1::Prepared,
        };
        // The mandatory launch audit is durable before the grant and any effect.
        let mut event = Event::new(LAUNCHED_KIND);
        event.id = bound_event_id(LAUNCH_EVENT_DOMAIN, request.operation_id);
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.session_id = Some(session_id.clone());
        event.data = serde_json::to_value(Launched {
            version: 1,
            operation_id: operation_id.clone(),
            principal_key: principal_key.clone(),
            membership_id: current.membership_id().to_string(),
            organization_scope: current.organization_scope().to_string(),
            launch_target_id: request.launch_target_id.to_string(),
            target_revision: target.revision,
            preparation_operation_id: preparation_operation_id.clone(),
            preparation_input_hash: request.preparation_input_hash.clone(),
            session_id: session_id.clone(),
            input_hash: input_hash.clone(),
            grant_operation_id: grant_operation_id.clone(),
            conversation: conversation.clone(),
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            "session launch requires redaction"
        );
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        // The real nominative session grant through the existing grant producer.
        // The owner is the authenticated initiator; ownership binds the session
        // to that human (a share never names the owner). The trusted control is
        // the effective actor, never a browser-supplied principal.
        let grant = ResourceGrantV1 {
            version: 1,
            tenant_id: self.tenant_id.clone(),
            organization_scope: current.organization_scope().to_string(),
            resource: ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: session_id.clone(),
            },
            owner: current.principal().clone(),
            revision: 1,
            active: true,
            shares: vec![],
        };
        let mutation = IdentityMutationV1 {
            version: 1,
            operation_id: grant_operation_id.clone(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: LAUNCH_GRANT_ACTOR.into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant,
            },
        };
        let grant_receipt = self.persist_identity_mutation(&tx, &mutation)?;
        let receipt = SessionLaunchReceiptV1 {
            version: 1,
            operation_id: request.operation_id,
            launch_id: stored.event.id.clone(),
            session_id: session_id.clone(),
            launch_target_id: request.launch_target_id,
            target_revision: target.revision,
            preparation_operation_id: request.preparation_operation_id,
            preparation_id: preparation_event.event.id.clone(),
            input_hash: input_hash.clone(),
            grant_revision: 1,
            grant_audit_seq: grant_receipt.audit_seq,
            grant_audit_hash: grant_receipt.audit_hash.clone(),
            audit_seq: stored.seq,
            audit_hash: stored.hash.clone(),
            conversation,
            replayed: false,
        };
        tx.execute(
            "INSERT INTO session_launches(operation_id,input_hash,session_id,preparation_operation_id,event_seq,event_hash,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                operation_id,
                input_hash,
                session_id,
                preparation_operation_id,
                stored.seq,
                stored.hash,
                serde_json::to_string(&receipt)?,
            ],
        )?;
        // Retain the conversation binding as the current channel anchor the
        // SendSessionMessage effect point revalidates. It starts `prepared`;
        // the trusted control binds the resolved channel explicitly later.
        tx.execute(
            "INSERT INTO session_conversations(session_id,binding_json,event_seq,event_hash) VALUES(?1,?2,?3,?4)",
            params![
                session_id,
                serde_json::to_string(&receipt.conversation)?,
                stored.seq,
                stored.hash,
            ],
        )?;
        // The additive first-binding grant keeps the authorization revision after
        // the human rights marker; reconcile the discovery generation exactly.
        let next = expected.after_delegated_change(
            human_modern::discovery_generation(&tx)?,
            grant_receipt.authorization_revision,
        )?;
        self.commit_authorized(tx, &clock)?;
        Ok((receipt, next))
    }

    /// Read one retained preparation receipt for its still-authorized initiator.
    /// Revocation of the identity, membership or context closes the read.
    pub(crate) fn modern_session_preparation(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        operation_id: &str,
    ) -> anyhow::Result<SessionPrepareReceiptV1> {
        crate::control::session_control::canonical_uuid(operation_id)
            .map_err(|_| failure(IdentityFailureV1::InvalidRequest))?;
        self.with_modern_read(access, |tx| {
            let current = authorize_prepare(tx, scope, &self.tenant_id)?;
            let row =
                preparation_row(tx, operation_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
            authorize_receipt_initiator(
                &current,
                &row.principal_key,
                &row.membership_id,
                &row.organization_scope,
            )?;
            let event = stored_event(tx, row.event_seq)?;
            anyhow::ensure!(event.hash == row.event_hash, IdentityFailureV1::Conflict);
            Ok(SessionPrepareReceiptV1 {
                version: 1,
                operation_id: crate::control::session_control::canonical_uuid(operation_id)?,
                preparation_id: event.event.id.clone(),
                launch_target_id: crate::control::session_control::canonical_uuid(
                    &row.launch_target_id,
                )?,
                target_revision: row.target_revision as u64,
                input_hash: row.input_hash.clone(),
                expires_at: row.expires_at.clone(),
                audit_seq: event.seq,
                audit_hash: event.hash.clone(),
                replayed: true,
            })
        })
    }
}

struct PreparationRow {
    input_hash: String,
    principal_key: String,
    membership_id: String,
    organization_scope: String,
    launch_target_id: String,
    target_revision: i64,
    expires_at: String,
    event_seq: i64,
    event_hash: String,
}

fn preparation_row(tx: &Connection, operation_id: &str) -> anyhow::Result<Option<PreparationRow>> {
    Ok(tx
        .query_row(
            "SELECT input_hash,principal_key,membership_id,organization_scope,launch_target_id,target_revision,expires_at,event_seq,event_hash FROM session_preparations WHERE operation_id=?1",
            [operation_id],
            |r| {
                Ok(PreparationRow {
                    input_hash: r.get(0)?,
                    principal_key: r.get(1)?,
                    membership_id: r.get(2)?,
                    organization_scope: r.get(3)?,
                    launch_target_id: r.get(4)?,
                    target_revision: r.get(5)?,
                    expires_at: r.get(6)?,
                    event_seq: r.get(7)?,
                    event_hash: r.get(8)?,
                })
            },
        )
        .optional()?)
}

struct LaunchRow {
    input_hash: String,
    event_seq: i64,
    event_hash: String,
    receipt_json: String,
}

fn launch_row(tx: &Connection, operation_id: &str) -> anyhow::Result<Option<LaunchRow>> {
    Ok(tx
        .query_row(
            "SELECT input_hash,event_seq,event_hash,receipt_json FROM session_launches WHERE operation_id=?1",
            [operation_id],
            |r| {
                Ok(LaunchRow {
                    input_hash: r.get(0)?,
                    event_seq: r.get(1)?,
                    event_hash: r.get(2)?,
                    receipt_json: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// The launch operation that already consumed one preparation, if any. One
/// retained preparation commits at most one launch; a second distinct launch
/// operation referencing it is a conflict, never a second session.
fn consumed_by_launch(
    tx: &Connection,
    preparation_operation_id: &str,
) -> anyhow::Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT operation_id FROM session_launches WHERE preparation_operation_id=?1",
            [preparation_operation_id],
            |r| r.get(0),
        )
        .optional()?)
}

struct TargetOperationRow {
    input_hash: String,
    event_seq: i64,
    event_hash: String,
}

fn target_operation(
    tx: &Connection,
    operation_id: &str,
) -> anyhow::Result<Option<TargetOperationRow>> {
    Ok(tx
        .query_row(
            "SELECT o.input_hash,o.event_seq,e.hash FROM session_launch_target_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
            [operation_id],
            |r| Ok(TargetOperationRow {
                input_hash: r.get(0)?,
                event_seq: r.get(1)?,
                event_hash: r.get(2)?,
            }),
        )
        .optional()?)
}

fn stored_event(tx: &Connection, seq: i64) -> anyhow::Result<StoredEvent> {
    Ok(tx.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [seq],
        row_event,
    )?)
}

fn preparation_receipt(
    request: &SessionLaunchPrepareRequestV1,
    event: &StoredEvent,
    replayed: bool,
) -> SessionPrepareReceiptV1 {
    SessionPrepareReceiptV1 {
        version: 1,
        operation_id: request.operation_id,
        preparation_id: event.event.id.clone(),
        launch_target_id: request.launch_target_id,
        target_revision: request.expected_target_revision,
        input_hash: request.input_hash().unwrap_or_default(),
        expires_at: request.expires_at.clone(),
        audit_seq: event.seq,
        audit_hash: event.hash.clone(),
        replayed,
    }
}

// Closed session launch target catalog; production wiring pending.
#[allow(dead_code)]
fn target_receipt(
    command: &ProvisionSessionLaunchTargetV1,
    event: &StoredEvent,
    hash: String,
    replayed: bool,
) -> SessionPrepareReceiptV1 {
    SessionPrepareReceiptV1 {
        version: 1,
        operation_id: command.operation_id,
        preparation_id: event.event.id.clone(),
        launch_target_id: command.target.launch_target_id,
        target_revision: command.target.revision,
        input_hash: command.input_hash().unwrap_or_default(),
        expires_at: String::new(),
        audit_seq: event.seq,
        audit_hash: hash,
        replayed,
    }
}

/// A UUIDv8 bound to the operation, distinct per record family so one operation
/// cannot collide across the preparation and its target provisioning.
pub(crate) fn bound_event_id(domain: &[u8], operation_id: Uuid) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(domain)
        .chain_update([0])
        .chain_update(operation_id.as_bytes())
        .finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .to_string()
}

fn preparation_event_id(operation_id: Uuid) -> String {
    bound_event_id(b"ai-guard:session-prepare:v1", operation_id)
}

// Closed session launch target catalog; production wiring pending.
#[allow(dead_code)]
fn provision_event_id(operation_id: Uuid) -> String {
    bound_event_id(b"ai-guard:session-launch-target:v1", operation_id)
}
