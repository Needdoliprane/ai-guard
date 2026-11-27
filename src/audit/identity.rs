//! Identity projections4 and explicit human-audit capability5, with atomic audit.
use super::*;
use crate::control::MigrationReportV1;
use crate::identity::*;

pub(crate) mod projection;
use projection::{Genesis, Projection, ProjectionCell, Record, conflict, invalid};

/// One bounded, transaction-consistent metadata observation. This is neither
/// an authorization capability nor a substitute for the audited human profile.
pub(crate) struct AuthenticationMembershipSnapshotV1 {
    pub account: Option<HumanAccountV1>,
    pub memberships: Vec<MembershipV1>,
    pub authorization_revision: u64,
}

#[cfg(all(test, unix))]
#[path = "identity_scoped_read_tests.rs"]
mod scoped_read_tests;

#[cfg(all(test, unix))]
#[path = "identity_authentication_metadata_tests.rs"]
mod authentication_metadata_tests;

#[cfg(all(test, unix))]
#[path = "identity_review_capability_tests.rs"]
mod review_capability_tests;

/// The approval capability enters a history only on the modern source formats
/// or, on a legacy store, after the human rights marker; never through a retry
/// that predates it. This is only a format fence, never approval authority.
fn require_approval_capability_format(
    mutation: &IdentityMutationV1,
    version: i64,
    rights_access: bool,
) -> anyhow::Result<()> {
    if !matches!(version, 14 | 15)
        && !rights_access
        && matches!(
            &mutation.change,
            IdentityChangeV1::Membership { membership, .. }
                if membership.capabilities.contains(&HumanCapabilityV1::ApproveLaunch)
        )
    {
        return Err(IdentityFailureV1::MigrationRequired.into());
    }
    Ok(())
}

pub(crate) fn require_replay_capabilities(
    mutation: &IdentityMutationV1,
    version: i64,
    broker_access: bool,
    worker_access: bool,
    rights_access: bool,
) -> anyhow::Result<()> {
    super::human_brokers::require_mutation_capability(mutation, broker_access)?;
    super::human_workers::require_mutation_capability(mutation, worker_access)?;
    super::human_rights::require_mutation_capability(mutation, rights_access)?;
    require_approval_capability_format(mutation, version, rights_access)
}

pub(super) const SCHEMA: [(&str, &str, &str); 11] = [
    (
        "table",
        "identity_meta",
        "CREATE TABLE identity_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),authorization_revision INTEGER NOT NULL CHECK(authorization_revision>=0),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "identity_accounts",
        "CREATE TABLE identity_accounts(principal_key TEXT PRIMARY KEY,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "table",
        "identity_credentials",
        "CREATE TABLE identity_credentials(credential_id TEXT PRIMARY KEY,principal_key TEXT NOT NULL REFERENCES identity_accounts(principal_key),digest TEXT NOT NULL UNIQUE,active INTEGER NOT NULL CHECK(active IN (0,1)),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "table",
        "identity_memberships",
        "CREATE TABLE identity_memberships(membership_id TEXT PRIMARY KEY,principal_key TEXT NOT NULL REFERENCES identity_accounts(principal_key),organization_scope TEXT NOT NULL,active INTEGER NOT NULL CHECK(active IN (0,1)),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),UNIQUE(principal_key,organization_scope))",
    ),
    (
        "table",
        "identity_resources",
        "CREATE TABLE identity_resources(resource_key TEXT PRIMARY KEY,resource_kind TEXT NOT NULL,resource_id TEXT NOT NULL,owner_key TEXT NOT NULL REFERENCES identity_accounts(principal_key),active INTEGER NOT NULL CHECK(active IN (0,1)),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),UNIQUE(resource_kind,resource_id))",
    ),
    (
        "table",
        "identity_shares",
        "CREATE TABLE identity_shares(resource_key TEXT NOT NULL REFERENCES identity_resources(resource_key),principal_key TEXT NOT NULL REFERENCES identity_accounts(principal_key),permissions_json TEXT NOT NULL,PRIMARY KEY(resource_key,principal_key))",
    ),
    (
        "table",
        "identity_operations",
        "CREATE TABLE identity_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "identity_credentials_principal",
        "CREATE INDEX identity_credentials_principal ON identity_credentials(principal_key)",
    ),
    (
        "index",
        "identity_memberships_principal",
        "CREATE INDEX identity_memberships_principal ON identity_memberships(principal_key,active)",
    ),
    (
        "index",
        "identity_resources_owner",
        "CREATE INDEX identity_resources_owner ON identity_resources(owner_key,active)",
    ),
    (
        "index",
        "identity_shares_principal",
        "CREATE INDEX identity_shares_principal ON identity_shares(principal_key,resource_key)",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 7] = [
    ("identity_meta", 1),
    ("identity_accounts", 1),
    ("identity_credentials", 2),
    ("identity_memberships", 2),
    ("identity_resources", 2),
    ("identity_shares", 1),
    ("identity_operations", 2),
];
const QUERIES: [(&str, &str); 7] = [
    (
        "identity_meta",
        "SELECT singleton,authorization_revision,genesis_seq,genesis_hash FROM identity_meta ORDER BY singleton",
    ),
    (
        "identity_accounts",
        "SELECT principal_key,payload_json,event_seq FROM identity_accounts ORDER BY principal_key",
    ),
    (
        "identity_credentials",
        "SELECT credential_id,principal_key,digest,active,payload_json,event_seq FROM identity_credentials ORDER BY credential_id",
    ),
    (
        "identity_memberships",
        "SELECT membership_id,principal_key,organization_scope,active,payload_json,event_seq FROM identity_memberships ORDER BY membership_id",
    ),
    (
        "identity_resources",
        "SELECT resource_key,resource_kind,resource_id,owner_key,active,payload_json,event_seq FROM identity_resources ORDER BY resource_key",
    ),
    (
        "identity_shares",
        "SELECT resource_key,principal_key,permissions_json FROM identity_shares ORDER BY resource_key,principal_key",
    ),
    (
        "identity_operations",
        "SELECT operation_id,input_hash,event_seq FROM identity_operations ORDER BY operation_id",
    ),
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanAccessGenesis {
    version: u32,
    protocol: String,
    control_namespace_uuid: String,
    identity_genesis_seq: i64,
    identity_genesis_hash: String,
    authorization_revision: u64,
    previous_seq: i64,
    previous_hash: String,
}
pub(super) fn require_identity(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}
// Identity-authority mutations prove their class before using this projection.
// Business readers enter through the separately checked read guards below;
// the legacy Connection-only guards retain their historical formats.
fn require_identity_projection(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}
// Closed human-access schema guard; production wiring pending.
#[allow(dead_code)]
pub(super) fn require_human_access(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}

/// Format 14 is usable only through an actual, live checked transaction that
/// retains its borrowed source authority through commit. A header alone and
/// the Connection-only legacy guards never acquire that authority.
pub(super) fn identity_read_format(tx: &CheckedTransaction<'_>) -> anyhow::Result<bool> {
    tx.require_business_access()?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok([4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version)
        || (matches!(version, 14 | 15) && tx.source_access.is_some()))
}

pub(super) fn require_identity_read(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
    if identity_read_format(tx)? {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}

pub(super) fn require_human_read(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
    tx.require_business_access()?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version)
        || (matches!(version, 14 | 15) && tx.source_access.is_some())
    {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}
pub(super) fn authorization_revision(db: &Connection) -> anyhow::Result<u64> {
    let revision: i64 = db.query_row(
        "SELECT authorization_revision FROM identity_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    u64::try_from(revision).context("negative identity authorization revision")
}

fn genesis(db: &Connection) -> anyhow::Result<StoredEvent> {
    let (seq, hash): (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let stored = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [seq],
        row_event,
    )?;
    let input: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    invalid(
        input.matches(&namespace, seq, &stored.prev_hash)
            && hash == stored.hash
            && stored.event.kind == "control.identity.genesis"
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "observed",
    )?;
    Ok(stored)
}
fn recorded(db: &Connection, id: &str) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let stored = db.query_row("SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload FROM identity_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",[id],row_event).optional()?;
    stored
        .map(|stored| {
            let record = serde_json::from_value(stored.event.data.clone())?;
            Ok((stored, record))
        })
        .transpose()
}

pub(super) fn require_recorded_mutation(
    db: &Connection,
    mutation: &IdentityMutationV1,
    expected: &IdentityReceiptV1,
) -> anyhow::Result<()> {
    let (stored, record) =
        recorded(db, &mutation.operation_id)?.ok_or(IdentityFailureV1::Conflict)?;
    conflict(record.mutation == *mutation && receipt(&stored, &record) == *expected)
}
fn receipt(stored: &StoredEvent, record: &Record) -> IdentityReceiptV1 {
    IdentityReceiptV1 {
        version: 1,
        operation_id: record.mutation.operation_id.clone(),
        input_hash: record.input_hash.clone(),
        authorization_revision: record.authorization_revision,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}

fn human_access_genesis(db: &Connection) -> anyhow::Result<(StoredEvent, HumanAccessGenesis)> {
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind='control.identity.human_access.genesis' ORDER BY seq LIMIT 2")?;
    let mut records = statement
        .query_map([], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    invalid(records.len() == 1)?;
    let stored = records.remove(0);
    let marker: HumanAccessGenesis = serde_json::from_value(stored.event.data.clone())?;
    let identity = genesis(db)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    invalid(
        marker.version == 5
            && marker.protocol == "local_human_access_v1"
            && marker.control_namespace_uuid == namespace
            && marker.identity_genesis_seq == identity.seq
            && marker.identity_genesis_hash == identity.hash
            && marker.previous_seq == stored.seq - 1
            && marker.previous_hash == stored.prev_hash
            && stored.seq > identity.seq
            && i64::try_from(marker.authorization_revision).is_ok()
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "observed",
    )?;
    Ok((stored, marker))
}
pub(super) fn validate_human_access_capability(
    db: &Connection,
    version: i64,
) -> anyhow::Result<()> {
    if [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        human_access_genesis(db)?;
    } else {
        let forbidden:bool = db.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE kind IN ('control.identity.access','control.identity.human_access.genesis'))",[],|r|r.get(0))?;
        invalid(!forbidden)?;
    }
    Ok(())
}

/// Read the revision of the identity prefix ending immediately before `seq`.
/// Full identity replay verifies the revisions carried by these mutations.
pub(super) fn authorization_revision_before(db: &Connection, seq: i64) -> anyhow::Result<u64> {
    let previous = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events \
             WHERE kind='control.identity.mutation' AND seq<?1 ORDER BY seq DESC LIMIT 1",
            [seq],
            row_event,
        )
        .optional()?;
    match previous {
        Some(stored) => Ok(Record::decode_replay(&stored.event.data)?.authorization_revision),
        None => Ok(0),
    }
}

impl Projection {
    pub(super) fn require_resource_at(
        &self,
        expected: &ResourceGrantV1,
        before_seq: i64,
    ) -> anyhow::Result<()> {
        invalid(
            self.resources
                .get(&expected.resource.key()?)
                .is_some_and(|(grant, seq)| grant == expected && *seq < before_seq),
        )
    }

    pub(super) fn require_local_launch_at(
        &self,
        principal: &PrincipalRefV1,
        (credential_id, credential_revision): (&str, u64),
        (membership_id, membership_revision): (&str, u64),
        (tenant, organization): (&str, &str),
        (before_seq, at): (i64, chrono::DateTime<chrono::Utc>),
    ) -> anyhow::Result<()> {
        let (account, account_seq) = self
            .accounts
            .get(&principal.key()?)
            .ok_or(IdentityFailureV1::IdentityRevoked)?;
        invalid(account.active && *account_seq < before_seq)?;
        let (credential, credential_seq) = self
            .credentials
            .get(credential_id)
            .ok_or(IdentityFailureV1::IdentityRevoked)?;
        let expiry = credential
            .expires_at
            .as_deref()
            .map(crate::broker_registry::parse_time)
            .transpose()?;
        invalid(
            credential.active
                && &credential.principal == principal
                && credential.revision == credential_revision
                && expiry.is_none_or(|deadline| at < deadline)
                && *credential_seq < before_seq,
        )?;
        let (member, member_seq) = self
            .memberships
            .get(membership_id)
            .ok_or(IdentityFailureV1::ContextRevoked)?;
        let scope = crate::identity::authorize_context(
            principal,
            member,
            ContextModeV1::Personal,
            membership_revision,
        )?;
        invalid(
            *member_seq < before_seq
                && scope.tenant_id() == tenant
                && scope.organization_scope() == organization
                && scope.has_capability(HumanCapabilityV1::LaunchSession),
        )
    }
}

/// Full replay is an integrity/startup operation, never a human read query.
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if ![4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        return Ok(());
    }
    let genesis = genesis(db)?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut projection = Projection::default();
    let mut statement = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events \
         WHERE kind GLOB 'control.identity.*' OR kind IN (?1,?2) ORDER BY seq",
    )?;
    let mut seen_genesis = false;
    let mut seen_human_access = false;
    let mut seen_broker_access = false;
    let mut seen_worker_access = false;
    let mut seen_rights_access = false;
    for row in statement.query_map(
        [
            legacy_session_prepare::ADMITTED,
            legacy_session_prepare::LEGACY_NAMED_ARM_KIND,
        ],
        row_event,
    )? {
        let stored = row?;
        if stored.event.kind == legacy_session_prepare::ADMITTED {
            invalid(seen_rights_access && legacy_session_prepare::format_admits(version))?;
            legacy_session_prepare::validate_admission_initiator(db, &stored, &projection)?;
            continue;
        }
        if stored.event.kind == legacy_session_prepare::LEGACY_NAMED_ARM_KIND {
            invalid(seen_rights_access && legacy_session_prepare::format_admits(version))?;
            legacy_session_prepare::validate_named_arm_initiator(db, &stored, &projection)?;
            continue;
        }
        if stored.event.kind == "control.identity.genesis" {
            invalid(!seen_genesis && stored.seq == genesis.seq && stored.hash == genesis.hash)?;
            seen_genesis = true;
            continue;
        }
        if stored.event.kind == "control.identity.human_access.genesis" {
            let (expected, marker) = human_access_genesis(db)?;
            invalid(
                [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)
                    && seen_genesis
                    && !seen_human_access
                    && stored.seq == expected.seq
                    && stored.hash == expected.hash
                    && marker.authorization_revision == projection.revision,
            )?;
            seen_human_access = true;
            continue;
        }
        if stored.event.kind == "control.identity.access" {
            invalid(
                [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)
                    && seen_genesis
                    && seen_human_access
                    && stored.event.source == "gateway"
                    && stored.event.provenance == "observed"
                    && stored.event.coverage == "observed",
            )?;
            let record: super::human::HumanReadAuditV1 =
                serde_json::from_value(stored.event.data.clone())?;
            super::human::validate_read(&record)?;
            invalid(
                !super::human_rights::read_action_requires_marker(&record.action)
                    || seen_rights_access,
            )?;
            invalid(record.authorization_revision == projection.revision)?;
            let account = projection
                .accounts
                .get(&record.principal.key()?)
                .ok_or(IdentityFailureV1::IdentityRevoked)?;
            invalid(account.0.active)?;
            if let Some(id) = record.membership_id.as_ref() {
                let member = projection
                    .memberships
                    .get(id)
                    .ok_or(IdentityFailureV1::ContextRevoked)?;
                let scope = crate::identity::authorize_context(
                    &record.principal,
                    &member.0,
                    record.mode.ok_or(IdentityFailureV1::InvalidRequest)?,
                    record
                        .membership_revision
                        .ok_or(IdentityFailureV1::InvalidRequest)?,
                )?;
                invalid(super::human_rights::read_action_permitted(
                    &record.action,
                    &scope,
                ))?;
            }
            continue;
        }
        if stored.event.kind == super::human_brokers::GENESIS_KIND {
            let (expected, revision) = super::human_brokers::genesis(db)?;
            invalid(
                [7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)
                    && seen_human_access
                    && !seen_broker_access
                    && stored.seq == expected.seq
                    && stored.hash == expected.hash
                    && revision == projection.revision,
            )?;
            seen_broker_access = true;
            continue;
        }
        if stored.event.kind == super::human_brokers::READ_KIND {
            invalid(
                [7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)
                    && seen_broker_access
                    && stored.event.source == "gateway"
                    && stored.event.provenance == "observed"
                    && stored.event.coverage == "observed",
            )?;
            let record: super::human_brokers::HumanBrokerReadAuditV1 =
                serde_json::from_value(stored.event.data.clone())?;
            super::human_brokers::validate_read(&record)?;
            invalid(record.authorization_revision == projection.revision)?;
            let account = projection
                .accounts
                .get(&record.principal.key()?)
                .ok_or(IdentityFailureV1::IdentityRevoked)?;
            invalid(account.0.active)?;
            let member = projection
                .memberships
                .get(&record.membership_id)
                .ok_or(IdentityFailureV1::ContextRevoked)?;
            let scope = crate::identity::authorize_context(
                &record.principal,
                &member.0,
                record.mode,
                record.membership_revision,
            )?;
            invalid(scope.has_capability(HumanCapabilityV1::ReadBrokerRegistry))?;
            continue;
        }
        if stored.event.kind == super::human_workers::GENESIS_KIND {
            let (expected, revision) = super::human_workers::genesis(db)?;
            invalid(
                matches!(version, 10..=15)
                    && seen_human_access
                    && seen_broker_access
                    && !seen_worker_access
                    && stored.seq == expected.seq
                    && stored.hash == expected.hash
                    && revision == projection.revision,
            )?;
            seen_worker_access = true;
            continue;
        }
        if stored.event.kind == super::human_rights::GENESIS_KIND {
            let (expected, revision) =
                super::human_rights::genesis(db)?.ok_or(IdentityFailureV1::InvalidRequest)?;
            invalid(
                super::human_rights::format_admits_marker(version)
                    && seen_worker_access
                    && !seen_rights_access
                    && stored.seq == expected.seq
                    && stored.hash == expected.hash
                    && revision == projection.revision,
            )?;
            seen_rights_access = true;
            projection.additive_resource_grants = true;
            continue;
        }
        if stored.event.kind == super::human_workers::READ_KIND {
            invalid(
                matches!(version, 10..=15)
                    && seen_worker_access
                    && stored.event.source == "gateway"
                    && stored.event.provenance == "observed"
                    && stored.event.coverage == "observed",
            )?;
            let record: super::human_workers::HumanWorkerReadAuditV1 =
                serde_json::from_value(stored.event.data.clone())?;
            super::human_workers::validate_read(&record)?;
            invalid(record.authorization_revision == projection.revision)?;
            let account = projection
                .accounts
                .get(&record.principal.key()?)
                .ok_or(IdentityFailureV1::IdentityRevoked)?;
            invalid(account.0.active)?;
            let member = projection
                .memberships
                .get(&record.membership_id)
                .ok_or(IdentityFailureV1::ContextRevoked)?;
            let scope = crate::identity::authorize_context(
                &record.principal,
                &member.0,
                record.mode,
                record.membership_revision,
            )?;
            invalid(scope.has_capability(HumanCapabilityV1::ReadWorkerRegistry))?;
            continue;
        }
        invalid(
            seen_genesis
                && stored.event.kind == "control.identity.mutation"
                && stored.event.source == "gateway"
                && stored.event.provenance == "observed"
                && stored.event.coverage == "observed",
        )?;
        let record = Record::decode_replay(&stored.event.data)?;
        require_replay_capabilities(
            &record.mutation,
            version,
            seen_broker_access,
            seen_worker_access,
            seen_rights_access,
        )?;
        projection.apply(&record.mutation, &tenant, stored.seq)?;
        ensure_resource_known(db, &record.mutation, stored.seq)?;
        record.verify_revision(&projection)?;
    }
    invalid(seen_genesis)?;
    invalid(([5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)) == seen_human_access)?;
    invalid(([7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)) == seen_broker_access)?;
    invalid((matches!(version, 10..=15)) == seen_worker_access)?;
    let mut expected = projection.rows(genesis.seq, &genesis.hash)?;
    for (table, query) in QUERIES {
        let mut statement = db.prepare(query)?;
        let width = statement.column_count();
        let actual = statement
            .query_map([], |r| {
                (0..width)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let expected_rows = expected.remove(table).map(|rows| {
            rows.into_iter()
                .map(|row| row.into_iter().map(rusqlite::types::Value::from).collect())
                .collect()
        });
        anyhow::ensure!(
            Some(actual) == expected_rows,
            "identity projection differs from audit: {table}"
        );
    }
    Ok(())
}

impl From<ProjectionCell> for rusqlite::types::Value {
    fn from(value: ProjectionCell) -> Self {
        match value {
            ProjectionCell::Integer(value) => Self::Integer(value),
            ProjectionCell::Text(value) => Self::Text(value),
        }
    }
}

// Mutation preflight reads only the accounts, memberships and unique bindings
// it can affect. The operation history is consulted by indexed operation_id,
// never loaded into a per-request snapshot.
fn load_projection(db: &Connection, mutation: &IdentityMutationV1) -> anyhow::Result<Projection> {
    let mut result = Projection::default();
    let organization = match &mutation.change {
        IdentityChangeV1::Membership { membership, .. } => {
            Some(membership.organization_scope.as_str())
        }
        IdentityChangeV1::ResourceGrant { grant, .. } => Some(grant.organization_scope.as_str()),
        _ => None,
    };
    if mutation.actor.is_human() {
        load_account(db, &mut result, &mutation.actor)?;
        if let Some(organization) = organization {
            load_member(db, &mut result, &mutation.actor, organization)?;
        }
    }
    match &mutation.change {
        IdentityChangeV1::Account { account, .. } => {
            load_account(db, &mut result, &account.principal)?
        }
        IdentityChangeV1::Credential { binding, .. } => {
            load_account(db, &mut result, &binding.principal)?;
            for (value, seq) in objects::<LocalCredentialV1>(
                db,
                "SELECT payload_json,event_seq FROM identity_credentials WHERE credential_id=?1 OR digest=?2",
                &[&binding.credential_id, &binding.sha256_digest],
            )? {
                result
                    .credentials
                    .insert(value.credential_id.clone(), (value, seq));
            }
        }
        IdentityChangeV1::Membership { membership, .. } => {
            load_account(db, &mut result, &membership.principal)?;
            load_member(
                db,
                &mut result,
                &membership.principal,
                &membership.organization_scope,
            )?;
            for (value, seq) in objects::<MembershipV1>(
                db,
                "SELECT payload_json,event_seq FROM identity_memberships WHERE membership_id=?1",
                &[&membership.membership_id],
            )? {
                result
                    .memberships
                    .insert(value.membership_id.clone(), (value, seq));
            }
        }
        IdentityChangeV1::ResourceGrant { grant, .. } => {
            load_account(db, &mut result, &grant.owner)?;
            load_member(db, &mut result, &grant.owner, &grant.organization_scope)?;
            for share in &grant.shares {
                load_account(db, &mut result, &share.principal)?;
                load_member(db, &mut result, &share.principal, &grant.organization_scope)?;
            }
            let key = grant.resource.key()?;
            for (value, seq) in objects::<ResourceGrantV1>(
                db,
                "SELECT payload_json,event_seq FROM identity_resources WHERE resource_key=?1",
                &[&key],
            )? {
                result.resources.insert(key.clone(), (value, seq));
            }
        }
    }
    result.revision = authorization_revision(db)?;
    Ok(result)
}

fn objects<T: serde::de::DeserializeOwned>(
    db: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> anyhow::Result<Vec<(T, i64)>> {
    let mut statement = db.prepare(sql)?;
    statement
        .query_map(args, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .map(|row| {
            let (payload, seq) = row?;
            Ok((serde_json::from_str(&payload)?, seq))
        })
        .collect()
}
fn load_account(
    db: &Connection,
    result: &mut Projection,
    principal: &PrincipalRefV1,
) -> anyhow::Result<()> {
    let key = principal.key()?;
    for (value, seq) in objects::<HumanAccountV1>(
        db,
        "SELECT payload_json,event_seq FROM identity_accounts WHERE principal_key=?1",
        &[&key],
    )? {
        result.accounts.insert(key.clone(), (value, seq));
    }
    Ok(())
}
fn load_member(
    db: &Connection,
    result: &mut Projection,
    principal: &PrincipalRefV1,
    organization: &str,
) -> anyhow::Result<()> {
    let key = principal.key()?;
    for (value, seq) in objects::<MembershipV1>(
        db,
        "SELECT payload_json,event_seq FROM identity_memberships WHERE principal_key=?1 AND organization_scope=?2",
        &[&key, &organization],
    )? {
        result
            .memberships
            .insert(value.membership_id.clone(), (value, seq));
    }
    Ok(())
}

fn ensure_resource_known(
    db: &Connection,
    mutation: &IdentityMutationV1,
    before: i64,
) -> anyhow::Result<()> {
    let IdentityChangeV1::ResourceGrant { grant, .. } = &mutation.change else {
        return Ok(());
    };
    let column = match grant.resource.kind {
        ResourceKindV1::Session => "json_extract(payload,'$.session_id')",
        ResourceKindV1::Agent => "agent_id",
        ResourceKindV1::Sandbox => "sandbox_id",
        ResourceKindV1::Project => "project_id",
    };
    let known: bool = db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM events WHERE seq<?1 AND {column}=?2 AND kind NOT GLOB 'control.identity.*')"),params![before,grant.resource.resource_id],|r|r.get(0))?;
    invalid(known)
}

impl Store {
    /// Explicit offline capability migration; no runtime lease activation.
    pub fn migrate_human_access_v5(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 4, 5)
    }
    pub(super) fn initialize_human_access(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        let identity = genesis(tx)?;
        let (previous_seq, previous_hash) = audit_head(tx)?;
        let namespace: String = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let marker = HumanAccessGenesis {
            version: 5,
            protocol: "local_human_access_v1".into(),
            control_namespace_uuid: namespace,
            identity_genesis_seq: identity.seq,
            identity_genesis_hash: identity.hash,
            authorization_revision: authorization_revision(tx)?,
            previous_seq,
            previous_hash,
        };
        let mut event = Event::new("control.identity.human_access.genesis");
        event.data = serde_json::to_value(marker)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        invalid(original == serde_json::to_value(&event)?)?;
        self.append_identity_in_transaction(tx, event, payload)?;
        Ok(())
    }
    /// Offline maintenance only, with an exclusive verified schema3 backup.
    pub fn migrate_identity_v4(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 3, 4)
    }
    pub(super) fn initialize_identity(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        for (_, _, sql) in SCHEMA {
            tx.execute_batch(sql)?;
        }
        let (previous_seq, previous_hash) = audit_head(tx)?;
        let namespace: String = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut event = Event::new("control.identity.genesis");
        event.data = serde_json::to_value(Genesis {
            version: 4,
            control_namespace_uuid: namespace,
            previous_seq,
            previous_hash,
        })?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        invalid(serde_json::to_value(&event)? == original)?;
        let stored = self.append_identity_in_transaction(tx, event, payload)?;
        tx.execute("INSERT INTO identity_meta(singleton,authorization_revision,genesis_seq,genesis_hash) VALUES(1,0,?1,?2)",params![stored.seq,stored.hash])?;
        Ok(())
    }
    /// Trusted control entry point; the caller authenticates the effective actor.
    /// No HTTP input or external claim can call this as a runner implicitly.
    pub fn mutate_identity(
        &self,
        mutation: &IdentityMutationV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        mutation.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        self.mutate_identity_checked(tx, mutation)
    }
    /// Modern trusted-control entrance. The epoch serializes authority changes;
    /// it does not authenticate the supplied actor or grant human approval.
    /// Startup/source14 validation remains mandatory before this can succeed.
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn mutate_identity_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        mutation: &IdentityMutationV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        mutation.validate()?;
        let access = HumanSourceAccess::write(epoch, source)?;
        // Never wait on a second control mutex while retaining a write epoch.
        // Both contention and poison refuse this operation without claiming a
        // durable source mismatch or releasing another task's owner.
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        self.mutate_identity_checked(tx, mutation)
    }
    pub(super) fn mutate_identity_checked(
        &self,
        mut tx: CheckedTransaction<'_>,
        mutation: &IdentityMutationV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        tx.require_identity_authority()?;
        require_identity_projection(&tx)?;
        let result = self.persist_identity_mutation(&tx, mutation)?;
        if matches!(&mutation.change, IdentityChangeV1::Credential { .. }) {
            tx.positive_bindings = Some(metadata::PositiveBindings::load(&tx)?);
        }
        self.commit_checked(tx)?;
        Ok(result)
    }

    /// Delegated changes of existing memberships and resource grants may advance
    /// the owned discovery snapshot. Their preparations check the actor's scope,
    /// and the changes preserve the credential and workload candidates.
    pub(super) fn mutate_delegation_with_discovery(
        &self,
        tx: CheckedTransaction<'_>,
        expected: HumanDiscoveryGeneration,
        prepare: impl FnOnce(&CheckedTransaction<'_>) -> anyhow::Result<IdentityMutationV1>,
    ) -> anyhow::Result<(IdentityReceiptV1, Option<HumanDiscoveryGeneration>)> {
        tx.require_identity_authority()?;
        require_identity_projection(&tx)?;
        if human_modern::discovery_generation(&tx)? != expected {
            return Err(HumanDiscoveryStale.into());
        }
        let mutation = prepare(&tx)?;
        invalid(
            mutation.actor.is_human()
                && matches!(
                    &mutation.change,
                    IdentityChangeV1::Membership { .. } | IdentityChangeV1::ResourceGrant { .. }
                ),
        )?;
        let receipt = self.persist_identity_mutation(&tx, &mutation)?;
        let next = expected.after_delegated_change(
            human_modern::discovery_generation(&tx)?,
            receipt.authorization_revision,
        )?;
        // A rejected or uncertain commit yields neither receipt nor transition.
        self.commit_checked(tx)?;
        Ok((receipt, next))
    }

    /// Atomically activate a prepared local account, credential and membership.
    /// Exact replay requires all three retained operation identities; partial
    /// overlap is a conflict and never an adoption of an existing account.
    pub fn provision_local_human(
        &self,
        provision: &LocalHumanProvisionV1,
    ) -> anyhow::Result<Vec<IdentityReceiptV1>> {
        provision.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        self.provision_local_human_checked(tx, provision)
    }
    /// Exact three-operation provisioning under the same write epoch and real
    /// SQLite transaction. No serialized scope or stopped-control assertion can
    /// construct the retained source/epoch needed by this entrance.
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn provision_local_human_scoped(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        provision: &LocalHumanProvisionV1,
    ) -> anyhow::Result<Vec<IdentityReceiptV1>> {
        provision.validate()?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        self.provision_local_human_checked(tx, provision)
    }
    fn provision_local_human_checked(
        &self,
        mut tx: CheckedTransaction<'_>,
        provision: &LocalHumanProvisionV1,
    ) -> anyhow::Result<Vec<IdentityReceiptV1>> {
        tx.require_identity_authority()?;
        provision.validate()?;
        require_identity_projection(&tx)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let rights_access = super::human_rights::installed(&tx)?;
        // Validate the complete atomic preparation before any replay lookup or
        // append, including the schema7 capability retained by schemas8–10 in its
        // third operation.
        for mutation in &provision.mutations {
            require_approval_capability_format(mutation, version, rights_access)?;
            super::human_brokers::require_mutation_capability(
                mutation,
                [7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
            )?;
            super::human_workers::require_mutation_capability(
                mutation,
                matches!(version, 10..=15),
            )?;
            super::human_rights::require_mutation_capability(mutation, rights_access)?;
        }
        let existing = provision
            .mutations
            .iter()
            .map(|m| recorded(&tx, &m.operation_id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let count = existing.iter().filter(|value| value.is_some()).count();
        conflict(count == 0 || count == 3)?;
        let receipts = provision
            .mutations
            .iter()
            .map(|mutation| self.persist_identity_mutation(&tx, mutation))
            .collect::<anyhow::Result<Vec<_>>>()?;
        tx.positive_bindings = Some(metadata::PositiveBindings::load(&tx)?);
        self.commit_checked(tx)?;
        Ok(receipts)
    }
    /// Persist one identity mutation inside a held identity-authority transaction
    /// without committing. The caller owns the atomic commit (e.g. the session
    /// launch, which appends its mandatory audit and retains its binding together
    /// with the nominative grant). Replay and projection rules are unchanged.
    pub(super) fn persist_identity_mutation(
        &self,
        tx: &CheckedTransaction<'_>,
        mutation: &IdentityMutationV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        // Keep the class/epoch check at the narrow mutation helper as well:
        // Deref to a raw SQLite transaction must not bypass Write authority.
        tx.require_identity_authority()?;
        mutation.validate()?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let rights_access = super::human_rights::installed(tx)?;
        require_approval_capability_format(mutation, version, rights_access)?;
        super::human_brokers::require_mutation_capability(
            mutation,
            [7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
        )?;
        super::human_workers::require_mutation_capability(mutation, matches!(version, 10..=15))?;
        super::human_rights::require_mutation_capability(mutation, rights_access)?;
        let value = serde_json::to_value(mutation)?;
        invalid(self.redactor.value(&value) == value)?;
        let hash = mutation.input_hash()?;
        if let Some((stored, record)) = recorded(tx, &mutation.operation_id)? {
            conflict(record.input_hash == hash && record.mutation == *mutation)?;
            return Ok(receipt(&stored, &record));
        }
        let next_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let mut projection = load_projection(tx, mutation)?;
        projection.additive_resource_grants = rights_access;
        let previous_revision = projection.revision;
        // Delegation rules bind new human-actor changes only; the historical
        // chain replays under the rules each record was accepted with.
        projection.check_holder_delegation(mutation)?;
        projection.check_delegation(mutation)?;
        projection.apply(mutation, &self.tenant_id, next_seq)?;
        ensure_resource_known(tx, mutation, next_seq)?;
        let record = Record {
            version: 1,
            authorization_revision: projection.revision,
            input_hash: hash,
            mutation: mutation.clone(),
        };
        let mut event = Event::new("control.identity.mutation");
        event.data = serde_json::to_value(&record)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        invalid(serde_json::to_value(&event)? == original)?;
        let stored = self.append_identity_in_transaction(tx, event, payload)?;
        conflict(stored.seq == next_seq)?;
        persist_change(tx, mutation, stored.seq)?;
        tx.execute(
            "INSERT INTO identity_operations(operation_id,input_hash,event_seq) VALUES(?1,?2,?3)",
            params![mutation.operation_id, record.input_hash, stored.seq],
        )?;
        if projection.revision != previous_revision {
            let next_revision = i64::try_from(projection.revision)
                .context("identity authorization revision exceeds SQLite integer range")?;
            let previous_revision = i64::try_from(previous_revision)
                .context("identity authorization revision exceeds SQLite integer range")?;
            conflict(next_revision.checked_sub(1) == Some(previous_revision))?;
            let updated = tx.execute("UPDATE identity_meta SET authorization_revision=?1 WHERE singleton=1 AND authorization_revision=?2",params![next_revision,previous_revision])?;
            conflict(updated == 1)?;
        }
        Ok(receipt(&stored, &record))
    }
    pub fn human_account(
        &self,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Option<HumanAccountV1>> {
        self.human_account_impl(None, principal)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn human_account_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Option<HumanAccountV1>> {
        self.human_account_impl(Some(access), principal)
    }
    fn human_account_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Option<HumanAccountV1>> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_identity_read(&tx)?;
        let value = json_row::<HumanAccountV1>(
            &tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?;
        self.commit_checked(tx)?;
        Ok(value)
    }
    pub(crate) fn authentication_membership_snapshot(
        &self,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<AuthenticationMembershipSnapshotV1> {
        self.authentication_membership_snapshot_impl(None, principal)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn authentication_membership_snapshot_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<AuthenticationMembershipSnapshotV1> {
        self.authentication_membership_snapshot_impl(Some(access), principal)
    }
    fn authentication_membership_snapshot_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<AuthenticationMembershipSnapshotV1> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_identity_read(&tx)?;
        let account = json_row::<HumanAccountV1>(
            &tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?;
        let authorization_revision = authorization_revision(&tx)?;
        let mut memberships = Vec::new();
        if let Some(account) = &account {
            anyhow::ensure!(
                account.principal == *principal,
                "authentication account metadata binding mismatch"
            );
            if account.active {
                // The sentinel bounds allocation before parsing a 257th row.
                let mut statement = tx.prepare(
                    "SELECT payload_json FROM identity_memberships WHERE principal_key=?1 ORDER BY membership_id LIMIT 257",
                )?;
                let rows = statement.query_map([key], |row| row.get::<_, String>(0))?;
                for row in rows {
                    anyhow::ensure!(
                        memberships.len() < 256,
                        "human membership capacity exceeded"
                    );
                    let membership: MembershipV1 = serde_json::from_str(&row?)?;
                    anyhow::ensure!(
                        membership.principal == *principal
                            && membership.tenant_id == self.tenant_id(),
                        "authentication membership metadata binding mismatch"
                    );
                    memberships.push(membership);
                }
            }
        }
        self.commit_checked(tx)?;
        Ok(AuthenticationMembershipSnapshotV1 {
            account,
            memberships,
            authorization_revision,
        })
    }
    pub fn identity_memberships(
        &self,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Vec<MembershipV1>> {
        self.identity_memberships_impl(None, principal)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn identity_memberships_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Vec<MembershipV1>> {
        self.identity_memberships_impl(Some(access), principal)
    }
    fn identity_memberships_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Vec<MembershipV1>> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_identity_read(&tx)?;
        let result = {
            let mut statement = tx.prepare("SELECT payload_json FROM identity_memberships WHERE principal_key=?1 ORDER BY membership_id")?;
            statement
                .query_map([key], |r| r.get::<_, String>(0))?
                .map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn identity_resource_grant(
        &self,
        resource: &ResourceRefV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        self.identity_resource_grant_impl(None, resource)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn identity_resource_grant_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        resource: &ResourceRefV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        self.identity_resource_grant_impl(Some(access), resource)
    }
    fn identity_resource_grant_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        resource: &ResourceRefV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        let key = resource.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_identity_read(&tx)?;
        let result = json_row(
            &tx,
            "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
            &key,
        )?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    /// The principal must already be authenticated; membership is reread atomically.
    pub fn authorize_human_context(
        &self,
        principal: &PrincipalRefV1,
        membership_id: &str,
        expected_revision: u64,
        mode: ContextModeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        self.authorize_human_context_impl(None, principal, membership_id, expected_revision, mode)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn authorize_human_context_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
        membership_id: &str,
        expected_revision: u64,
        mode: ContextModeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        self.authorize_human_context_impl(
            Some(access),
            principal,
            membership_id,
            expected_revision,
            mode,
        )
    }
    fn authorize_human_context_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        principal: &PrincipalRefV1,
        membership_id: &str,
        expected_revision: u64,
        mode: ContextModeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let result = self.authorize_human_context_checked(
            &tx,
            principal,
            &key,
            membership_id,
            expected_revision,
            mode,
        )?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub(super) fn authorize_human_context_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        principal: &PrincipalRefV1,
        key: &str,
        membership_id: &str,
        expected_revision: u64,
        mode: ContextModeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        require_identity_read(tx)?;
        let account: HumanAccountV1 = json_row(
            tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            key,
        )?
        .ok_or(IdentityFailureV1::IdentityRevoked)?;
        if !account.active {
            return Err(IdentityFailureV1::IdentityRevoked.into());
        }
        let membership: MembershipV1 = json_row(
            tx,
            "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
            membership_id,
        )?
        .ok_or(IdentityFailureV1::ContextRevoked)?;
        invalid(membership.tenant_id == self.tenant_id)?;
        let authorization_revision = authorization_revision(tx)?;
        let scope =
            crate::identity::authorize_context(principal, &membership, mode, expected_revision)?
                .at_authorization_revision(authorization_revision);
        Ok(scope)
    }
    /// Revalidate account, context revision and all authorization mutations in
    /// the same checked transaction as the resource lookup. None is uniform for
    /// an absent or inaccessible resource; stale contexts require reacquisition.
    pub fn authorized_resource_grant(
        &self,
        scope: &AuthorizedScopeV1,
        resource: &ResourceRefV1,
        permission: ResourcePermissionV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        self.authorized_resource_grant_impl(None, scope, resource, permission)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn authorized_resource_grant_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        resource: &ResourceRefV1,
        permission: ResourcePermissionV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        self.authorized_resource_grant_impl(Some(access), scope, resource, permission)
    }
    fn authorized_resource_grant_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        scope: &AuthorizedScopeV1,
        resource: &ResourceRefV1,
        permission: ResourcePermissionV1,
    ) -> anyhow::Result<Option<ResourceGrantV1>> {
        let resource_key = resource.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let current = revalidate_scope_checked(&tx, scope, &self.tenant_id)?;
        let result: Option<ResourceGrantV1> = json_row(
            &tx,
            "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
            &resource_key,
        )?;
        let result = result.filter(|grant| current.permits_resource(grant, permission));
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn identity_authorization_revision(&self) -> anyhow::Result<u64> {
        self.identity_authorization_revision_impl(None)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn identity_authorization_revision_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
    ) -> anyhow::Result<u64> {
        self.identity_authorization_revision_impl(Some(access))
    }
    fn identity_authorization_revision_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
    ) -> anyhow::Result<u64> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_identity_read(&tx)?;
        let revision = authorization_revision(&tx)?;
        self.commit_checked(tx)?;
        Ok(revision)
    }
}

pub(super) fn revalidate_scope(
    db: &Connection,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    require_identity(db)?;
    revalidate_scope_projection(db, scope, tenant)
}

pub(super) fn revalidate_scope_checked(
    tx: &CheckedTransaction<'_>,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    require_identity_read(tx)?;
    revalidate_scope_projection(tx, scope, tenant)
}

fn revalidate_scope_projection(
    db: &Connection,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    if scope.tenant_id() != tenant || authorization_revision(db)? != scope.authorization_revision()
    {
        return Err(IdentityFailureV1::ContextRevoked.into());
    }
    let account: HumanAccountV1 = json_row(
        db,
        "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
        &scope.principal().key()?,
    )?
    .ok_or(IdentityFailureV1::IdentityRevoked)?;
    if !account.active {
        return Err(IdentityFailureV1::IdentityRevoked.into());
    }
    let member: MembershipV1 = json_row(
        db,
        "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
        scope.membership_id(),
    )?
    .ok_or(IdentityFailureV1::ContextRevoked)?;
    let current = crate::identity::authorize_context(
        scope.principal(),
        &member,
        scope.mode(),
        scope.membership_revision(),
    )?
    .at_authorization_revision(scope.authorization_revision());
    Ok(scope.retain_ceiling_on_revalidation(current)?)
}

pub(super) fn json_row<T: serde::de::DeserializeOwned>(
    db: &Connection,
    sql: &str,
    key: &str,
) -> anyhow::Result<Option<T>> {
    db.query_row(sql, [key], |r| r.get::<_, String>(0))
        .optional()?
        .map(|p| serde_json::from_str(&p).map_err(Into::into))
        .transpose()
}
fn persist_change(
    tx: &Transaction<'_>,
    mutation: &IdentityMutationV1,
    seq: i64,
) -> anyhow::Result<()> {
    match &mutation.change {
        IdentityChangeV1::Account { account, .. } => {
            tx.execute("INSERT INTO identity_accounts(principal_key,payload_json,event_seq) VALUES(?1,?2,?3) ON CONFLICT(principal_key) DO UPDATE SET payload_json=excluded.payload_json,event_seq=excluded.event_seq",params![account.principal.key()?,serde_json::to_string(account)?,seq])?;
        }
        IdentityChangeV1::Credential { binding: c, .. } => {
            tx.execute("INSERT INTO identity_credentials(credential_id,principal_key,digest,active,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(credential_id) DO UPDATE SET digest=excluded.digest,active=excluded.active,payload_json=excluded.payload_json,event_seq=excluded.event_seq",params![c.credential_id,c.principal.key()?,c.sha256_digest,c.active,serde_json::to_string(c)?,seq])?;
        }
        IdentityChangeV1::Membership { membership: m, .. } => {
            tx.execute("INSERT INTO identity_memberships(membership_id,principal_key,organization_scope,active,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(membership_id) DO UPDATE SET active=excluded.active,payload_json=excluded.payload_json,event_seq=excluded.event_seq",params![m.membership_id,m.principal.key()?,m.organization_scope,m.active,serde_json::to_string(m)?,seq])?;
        }
        IdentityChangeV1::ResourceGrant { grant: g, .. } => {
            let key = g.resource.key()?;
            let kind = serde_json::to_value(g.resource.kind)?
                .as_str()
                .context("invalid resource kind")?
                .to_owned();
            tx.execute("INSERT INTO identity_resources(resource_key,resource_kind,resource_id,owner_key,active,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(resource_key) DO UPDATE SET owner_key=excluded.owner_key,active=excluded.active,payload_json=excluded.payload_json,event_seq=excluded.event_seq",params![key,kind,g.resource.resource_id,g.owner.key()?,g.active,serde_json::to_string(g)?,seq])?;
            tx.execute("DELETE FROM identity_shares WHERE resource_key=?1", [&key])?;
            for share in &g.shares {
                tx.execute("INSERT INTO identity_shares(resource_key,principal_key,permissions_json) VALUES(?1,?2,?3)",params![key,share.principal.key()?,serde_json::to_string(&share.permissions)?])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // Real SQLite allocation failure after some DDL must leave no partial4
    // objects, no new audit event, and no recreated or lost live witness.
    #[test]
    fn sqlite_full_during_identity_ddl_rolls_back_the_exact_schema_three() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory.path().join("audit.sqlite");
        let backup = directory.path().join("before-v4.sqlite");
        let store = Store::create_supervised_for_tenant(
            &path,
            "tenant-a",
            Redactor::new(vec![]),
            1000,
            &directory.path().join("before-v3.sqlite"),
        )
        .unwrap();
        let head = store.head().unwrap();
        {
            let db = store.db.lock().unwrap();
            let pages: i64 = db
                .pragma_query_value(None, "page_count", |r| r.get(0))
                .unwrap();
            db.pragma_update(None, "max_page_count", pages + 4).unwrap();
        }
        let error = store.migrate_identity_v4(&backup).unwrap_err();
        assert!(
            matches!(error.downcast_ref::<rusqlite::Error>(),Some(rusqlite::Error::SqliteFailure(code,_)) if code.code == rusqlite::ErrorCode::DiskFull)
        );
        let db = store.db.lock().unwrap();
        assert_eq!(
            db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            3
        );
        assert_eq!(
            db.query_row::<i64, _, _>(
                "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'identity_*'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        drop(db);
        assert_eq!(store.head().unwrap(), head);
        assert!(store.supervision_state().unwrap().active);
        assert!(store.integrity().unwrap().valid);
        let backed =
            Store::open_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
        assert_eq!(backed.supervision_state().unwrap().schema_version, 3);
        assert_eq!(backed.head().unwrap(), head);
    }
}
