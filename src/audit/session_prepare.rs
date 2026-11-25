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
    ProvisionSessionLaunchTargetV1, SessionLaunchPrepareRequestV1, SessionLaunchTargetRecordV1,
    SessionPrepareReceiptV1,
};
use crate::identity::{AuthorizedScopeV1, ContextModeV1, HumanCapabilityV1, IdentityFailureV1};
use uuid::Uuid;

pub(crate) const GENESIS_KIND: &str = "control.session_prepare.genesis";
pub(crate) const PREPARED_KIND: &str = "control.session_prepare.prepared";
const TARGET_PROVISION_KIND: &str = "control.session_prepare.target_provisioned";
const PROTOCOL: &str = "session_prepare_v1";
const MARKER_VERSION: u32 = 1;

const TARGETS_TABLE: &str = "session_launch_targets";
const TARGET_OPERATIONS_TABLE: &str = "session_launch_target_operations";
const PREPARATIONS_TABLE: &str = "session_preparations";

const TARGETS_SCHEMA: &str = "CREATE TABLE session_launch_targets(launch_target_id TEXT PRIMARY KEY,revision INTEGER NOT NULL,tenant_id TEXT NOT NULL,organization_scope TEXT NOT NULL,active INTEGER NOT NULL CHECK(active IN (0,1)),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))";
const TARGET_OPERATIONS_SCHEMA: &str = "CREATE TABLE session_launch_target_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))";
const PREPARATIONS_SCHEMA: &str = "CREATE TABLE session_preparations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,principal_key TEXT NOT NULL,membership_id TEXT NOT NULL,organization_scope TEXT NOT NULL,launch_target_id TEXT NOT NULL,target_revision INTEGER NOT NULL,expires_at TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL)";

pub(crate) const SCHEMA: [(&str, &str, &str); 3] = [
    ("table", TARGETS_TABLE, TARGETS_SCHEMA),
    ("table", TARGET_OPERATIONS_TABLE, TARGET_OPERATIONS_SCHEMA),
    ("table", PREPARATIONS_TABLE, PREPARATIONS_SCHEMA),
];
pub(crate) const AUTO_INDEX_COUNTS: [(&str, u32); 3] = [
    (TARGETS_TABLE, 1),
    (TARGET_OPERATIONS_TABLE, 1),
    (PREPARATIONS_TABLE, 1),
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
pub(crate) fn validate_capability(db: &Connection, version: i64) -> anyhow::Result<()> {
    let present = capability_present(db)?;
    let tables = table_present(db, TARGETS_TABLE)?
        && table_present(db, TARGET_OPERATIONS_TABLE)?
        && table_present(db, PREPARATIONS_TABLE)?;
    if present || tables {
        anyhow::ensure!(
            format_admits(version) && present && tables,
            "session prepare capability requires its marker and tables on Store14/15"
        );
        genesis(db)?;
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

/// Replay validation of one `control.session_prepare.*` event inside the
/// closed control replay. The marker revalidates against its durable prefix;
/// the preparation and catalog payloads are closed records. None of them may
/// carry a session id: the preparation uses its own identifier.
pub(crate) fn validate_replay_event(db: &Connection, stored: &StoredEvent) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native"
            && stored.event.session_id.is_none(),
        "unexpected session prepare event provenance"
    );
    match stored.event.kind.as_str() {
        GENESIS_KIND => {
            anyhow::ensure!(genesis(db)?.is_some(), "invalid session prepare marker");
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

/// Resolve one active catalog target and check the exact selection against it.
fn authorize_target(
    tx: &CheckedTransaction<'_>,
    current: &AuthorizedScopeV1,
    request: &SessionLaunchPrepareRequestV1,
) -> anyhow::Result<SessionLaunchTargetRecordV1> {
    let row: Option<String> = tx
        .query_row(
            "SELECT payload_json FROM session_launch_targets WHERE launch_target_id=?1",
            [request.launch_target_id.to_string()],
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
        selection: crate::control::session_control::SessionLaunchTargetSelectionV1::new(
            target.launch_target_id,
            target.revision,
        )?,
        active: target.active,
        tenant_id: &target.tenant_id,
        organization_scope: &target.organization_scope,
        launchers: &target.launchers,
    };
    policy
        .authorize_launch(current, request.selection()?)
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
        if let Some((seq, hash)) = target_operation(&tx, &operation_id)? {
            let stored_hash: String = tx.query_row(
                "SELECT input_hash FROM session_launch_target_operations WHERE operation_id=?1",
                [&operation_id],
                |r| r.get(0),
            )?;
            anyhow::ensure!(stored_hash == input_hash, IdentityFailureV1::Conflict);
            let event = stored_event(&tx, seq)?;
            return Ok(target_receipt(command, &event, hash, true));
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
    /// returns the original receipt to a still-authorized caller; a divergent
    /// intent under the same operation is a conflict; revocation closes it.
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
            anyhow::ensure!(row.input_hash == input_hash, IdentityFailureV1::Conflict);
            let event = stored_event(&tx, row.event_seq)?;
            anyhow::ensure!(event.hash == row.event_hash, IdentityFailureV1::Conflict);
            return Ok((preparation_receipt(request, &event, true), None));
        }
        // A new preparation resolves the current catalog target; an exact replay
        // above serves the retained receipt even if the target has since advanced.
        let target = authorize_target(&tx, &current, request)?;
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

    /// Read one retained preparation receipt for a still-authorized caller.
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
            let _current = authorize_prepare(tx, scope, &self.tenant_id)?;
            let row =
                preparation_row(tx, operation_id)?.ok_or(failure(IdentityFailureV1::NotFound))?;
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
    launch_target_id: String,
    target_revision: i64,
    expires_at: String,
    event_seq: i64,
    event_hash: String,
}

fn preparation_row(tx: &Connection, operation_id: &str) -> anyhow::Result<Option<PreparationRow>> {
    Ok(tx
        .query_row(
            "SELECT input_hash,launch_target_id,target_revision,expires_at,event_seq,event_hash FROM session_preparations WHERE operation_id=?1",
            [operation_id],
            |r| {
                Ok(PreparationRow {
                    input_hash: r.get(0)?,
                    launch_target_id: r.get(1)?,
                    target_revision: r.get(2)?,
                    expires_at: r.get(3)?,
                    event_seq: r.get(4)?,
                    event_hash: r.get(5)?,
                })
            },
        )
        .optional()?)
}

fn target_operation(tx: &Connection, operation_id: &str) -> anyhow::Result<Option<(i64, String)>> {
    Ok(tx
        .query_row(
            "SELECT event_seq,hash FROM session_launch_target_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
            [operation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
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
fn bound_event_id(domain: &[u8], operation_id: Uuid) -> String {
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

fn provision_event_id(operation_id: Uuid) -> String {
    bound_event_id(b"ai-guard:session-launch-target:v1", operation_id)
}
