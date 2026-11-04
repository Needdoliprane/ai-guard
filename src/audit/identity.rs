//! Identity projections4 and explicit human-audit capability5, with atomic audit.
use super::*;
use crate::control::MigrationReportV1;
use crate::identity::*;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    control_namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    authorization_revision: u64,
    input_hash: String,
    mutation: IdentityMutationV1,
}
#[derive(Default)]
struct Projection {
    accounts: BTreeMap<String, (HumanAccountV1, i64)>,
    credentials: BTreeMap<String, (LocalCredentialV1, i64)>,
    memberships: BTreeMap<String, (MembershipV1, i64)>,
    resources: BTreeMap<String, (ResourceGrantV1, i64)>,
    operations: BTreeMap<String, (String, i64)>,
    revision: u64,
}
type SqlRow = Vec<rusqlite::types::Value>;
type ProjectionRows = BTreeMap<&'static str, Vec<SqlRow>>;

fn invalid(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(IdentityFailureV1::InvalidRequest.into())
    }
}
fn conflict(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(IdentityFailureV1::Conflict.into())
    }
}
pub(super) fn require_identity(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [4, 5, 6].contains(&version) {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}
pub(super) fn require_human_access(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [5, 6].contains(&version) {
        Ok(())
    } else {
        Err(IdentityFailureV1::MigrationRequired.into())
    }
}
fn revision(previous: Option<u64>, expected: u64, next: u64) -> anyhow::Result<()> {
    conflict(previous.unwrap_or(0) == expected)?;
    invalid(expected.checked_add(1) == Some(next))
}
pub(super) fn authorization_revision(db: &Connection) -> anyhow::Result<u64> {
    let revision: i64 = db.query_row(
        "SELECT authorization_revision FROM identity_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    u64::try_from(revision).context("negative identity authorization revision")
}

impl Projection {
    fn account_exists(&self, principal: &PrincipalRefV1) -> anyhow::Result<()> {
        invalid(self.accounts.contains_key(&principal.key()?))
    }
    fn member_exists(&self, principal: &PrincipalRefV1, organization: &str) -> bool {
        self.memberships.values().any(|(m, _)| {
            &m.principal == principal && m.active && m.organization_scope == organization
        })
    }
    fn apply(
        &mut self,
        mutation: &IdentityMutationV1,
        tenant: &str,
        seq: i64,
    ) -> anyhow::Result<()> {
        mutation.validate()?;
        invalid(!self.operations.contains_key(&mutation.operation_id))?;
        match &mutation.actor {
            PrincipalRefV1::LegacyControl {
                role: LegacyControlRoleV1::Runner,
                ..
            } => {}
            principal if principal.is_human() => {
                let actor = self
                    .accounts
                    .get(&principal.key()?)
                    .ok_or(IdentityFailureV1::PermissionDenied)?;
                invalid(actor.0.active)?;
                let (capability, organization) = match &mutation.change {
                    IdentityChangeV1::ResourceGrant { grant, .. } => {
                        (HumanCapabilityV1::ManageGrants, &grant.organization_scope)
                    }
                    IdentityChangeV1::Membership { membership, .. } => (
                        HumanCapabilityV1::ManageMemberships,
                        &membership.organization_scope,
                    ),
                    // Accounts and credentials are provisioned by the trusted
                    // operator in this schema foundation, not a tenant role.
                    _ => return Err(IdentityFailureV1::PermissionDenied.into()),
                };
                if !self.memberships.values().any(|(m, _)| {
                    m.active
                        && &m.principal == principal
                        && m.organization_scope == *organization
                        && m.capabilities.contains(&capability)
                }) {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
            }
            _ => return Err(IdentityFailureV1::PermissionDenied.into()),
        }
        match &mutation.change {
            IdentityChangeV1::Account {
                expected_revision,
                account,
            } => {
                let key = account.principal.key()?;
                revision(
                    self.accounts.get(&key).map(|a| a.0.revision),
                    *expected_revision,
                    account.revision,
                )?;
                self.accounts.insert(key, (account.clone(), seq));
            }
            IdentityChangeV1::Credential {
                expected_revision,
                binding: credential,
            } => {
                self.account_exists(&credential.principal)?;
                let old = self.credentials.get(&credential.credential_id);
                revision(
                    old.map(|c| c.0.revision),
                    *expected_revision,
                    credential.revision,
                )?;
                invalid(old.is_none_or(|c| c.0.principal == credential.principal))?;
                conflict(!self.credentials.values().any(|(c, _)| {
                    c.sha256_digest == credential.sha256_digest
                        && c.credential_id != credential.credential_id
                }))?;
                self.credentials
                    .insert(credential.credential_id.clone(), (credential.clone(), seq));
            }
            IdentityChangeV1::Membership {
                expected_revision,
                membership,
            } => {
                invalid(membership.tenant_id == tenant)?;
                self.account_exists(&membership.principal)?;
                let old = self.memberships.get(&membership.membership_id);
                revision(
                    old.map(|m| m.0.revision),
                    *expected_revision,
                    membership.revision,
                )?;
                invalid(old.is_none_or(|m| {
                    m.0.principal == membership.principal
                        && m.0.organization_scope == membership.organization_scope
                }))?;
                conflict(!self.memberships.values().any(|(m, _)| {
                    m.principal == membership.principal
                        && m.organization_scope == membership.organization_scope
                        && m.membership_id != membership.membership_id
                }))?;
                self.memberships
                    .insert(membership.membership_id.clone(), (membership.clone(), seq));
            }
            IdentityChangeV1::ResourceGrant {
                expected_revision,
                grant,
            } => {
                let key = grant.resource.key()?;
                // Only the operator may establish a resource's first binding.
                // A tombstone remains an assigned resource, retaining its org.
                if mutation.actor.is_human()
                    && self.resources.get(&key).is_none_or(|previous| {
                        previous.0.organization_scope != grant.organization_scope
                    })
                {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
                invalid(grant.tenant_id == tenant)?;
                self.account_exists(&grant.owner)?;
                invalid(self.member_exists(&grant.owner, &grant.organization_scope))?;
                for share in &grant.shares {
                    self.account_exists(&share.principal)?;
                    invalid(self.member_exists(&share.principal, &grant.organization_scope))?;
                }
                invalid(
                    self.resources
                        .get(&key)
                        .is_none_or(|g| g.0.organization_scope == grant.organization_scope),
                )?;
                revision(
                    self.resources.get(&key).map(|g| g.0.revision),
                    *expected_revision,
                    grant.revision,
                )?;
                self.resources.insert(key, (grant.clone(), seq));
            }
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(IdentityFailureV1::InvalidRequest)?;
        i64::try_from(self.revision)
            .context("identity authorization revision exceeds SQLite integer range")?;
        self.operations
            .insert(mutation.operation_id.clone(), (mutation.input_hash()?, seq));
        Ok(())
    }
    fn rows(&self, genesis: &StoredEvent) -> anyhow::Result<ProjectionRows> {
        use rusqlite::types::Value as Sql;
        let text = |s: String| Sql::Text(s);
        let int = |i: i64| Sql::Integer(i);
        let mut result: BTreeMap<_, Vec<_>> =
            QUERIES.iter().map(|(t, _)| (*t, Vec::new())).collect();
        result
            .get_mut("identity_meta")
            .context("missing identity table")?
            .push(vec![
                int(1),
                int(i64::try_from(self.revision)?),
                int(genesis.seq),
                text(genesis.hash.clone()),
            ]);
        for (key, (account, seq)) in &self.accounts {
            result
                .get_mut("identity_accounts")
                .context("missing identity table")?
                .push(vec![
                    text(key.clone()),
                    text(serde_json::to_string(account)?),
                    int(*seq),
                ]);
        }
        for (id, (c, seq)) in &self.credentials {
            result
                .get_mut("identity_credentials")
                .context("missing identity table")?
                .push(vec![
                    text(id.clone()),
                    text(c.principal.key()?),
                    text(c.sha256_digest.clone()),
                    int(i64::from(c.active)),
                    text(serde_json::to_string(c)?),
                    int(*seq),
                ]);
        }
        for (id, (m, seq)) in &self.memberships {
            result
                .get_mut("identity_memberships")
                .context("missing identity table")?
                .push(vec![
                    text(id.clone()),
                    text(m.principal.key()?),
                    text(m.organization_scope.clone()),
                    int(i64::from(m.active)),
                    text(serde_json::to_string(m)?),
                    int(*seq),
                ]);
        }
        for (key, (g, seq)) in &self.resources {
            let kind = serde_json::to_value(g.resource.kind)?
                .as_str()
                .context("invalid resource kind")?
                .to_owned();
            result
                .get_mut("identity_resources")
                .context("missing identity table")?
                .push(vec![
                    text(key.clone()),
                    text(kind),
                    text(g.resource.resource_id.clone()),
                    text(g.owner.key()?),
                    int(i64::from(g.active)),
                    text(serde_json::to_string(g)?),
                    int(*seq),
                ]);
            let mut shares = g
                .shares
                .iter()
                .map(|s| Ok((s.principal.key()?, serde_json::to_string(&s.permissions)?)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            shares.sort();
            for (principal, permissions) in shares {
                result
                    .get_mut("identity_shares")
                    .context("missing identity table")?
                    .push(vec![text(key.clone()), text(principal), text(permissions)]);
            }
        }
        for (id, (hash, seq)) in &self.operations {
            result
                .get_mut("identity_operations")
                .context("missing identity table")?
                .push(vec![text(id.clone()), text(hash.clone()), int(*seq)]);
        }
        Ok(result)
    }
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
        input.version == 4
            && input.control_namespace_uuid == namespace
            && input.previous_seq == seq - 1
            && input.previous_hash == stored.prev_hash
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
    if [5, 6].contains(&version) {
        human_access_genesis(db)?;
    } else {
        let forbidden:bool = db.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE kind IN ('control.identity.access','control.identity.human_access.genesis'))",[],|r|r.get(0))?;
        invalid(!forbidden)?;
    }
    Ok(())
}

/// Full replay is an integrity/startup operation, never a human read query.
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if ![4, 5, 6].contains(&version) {
        return Ok(());
    }
    let genesis = genesis(db)?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut projection = Projection::default();
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.identity.*' ORDER BY seq")?;
    let mut seen_genesis = false;
    let mut seen_human_access = false;
    for row in statement.query_map([], row_event)? {
        let stored = row?;
        if stored.event.kind == "control.identity.genesis" {
            invalid(!seen_genesis && stored.seq == genesis.seq && stored.hash == genesis.hash)?;
            seen_genesis = true;
            continue;
        }
        if stored.event.kind == "control.identity.human_access.genesis" {
            let (expected, marker) = human_access_genesis(db)?;
            invalid(
                [5, 6].contains(&version)
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
                [5, 6].contains(&version)
                    && seen_genesis
                    && seen_human_access
                    && stored.event.source == "gateway"
                    && stored.event.provenance == "observed"
                    && stored.event.coverage == "observed",
            )?;
            let record: super::human::HumanReadAuditV1 =
                serde_json::from_value(stored.event.data.clone())?;
            super::human::validate_read(&record)?;
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
                crate::identity::authorize_context(
                    &record.principal,
                    &member.0,
                    record.mode.ok_or(IdentityFailureV1::InvalidRequest)?,
                    record
                        .membership_revision
                        .ok_or(IdentityFailureV1::InvalidRequest)?,
                )?;
            }
            continue;
        }
        invalid(
            seen_genesis
                && stored.event.kind == "control.identity.mutation"
                && stored.event.source == "gateway"
                && stored.event.provenance == "observed"
                && stored.event.coverage == "observed",
        )?;
        let record: Record = serde_json::from_value(stored.event.data.clone())?;
        invalid(record.version == 1 && record.input_hash == record.mutation.input_hash()?)?;
        projection.apply(&record.mutation, &tenant, stored.seq)?;
        ensure_resource_known(db, &record.mutation, stored.seq)?;
        invalid(record.authorization_revision == projection.revision)?;
    }
    invalid(seen_genesis)?;
    invalid(([5, 6].contains(&version)) == seen_human_access)?;
    let mut expected = projection.rows(&genesis)?;
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
        anyhow::ensure!(
            Some(actual) == expected.remove(table),
            "identity projection differs from audit: {table}"
        );
    }
    Ok(())
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
        require_identity(&tx)?;
        let result = self.persist_identity_mutation(&tx, mutation)?;
        self.commit_checked(tx)?;
        Ok(result)
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
        require_identity(&tx)?;
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
        self.commit_checked(tx)?;
        Ok(receipts)
    }
    fn persist_identity_mutation(
        &self,
        tx: &Transaction<'_>,
        mutation: &IdentityMutationV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        mutation.validate()?;
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
        let next_revision = i64::try_from(projection.revision)
            .context("identity authorization revision exceeds SQLite integer range")?;
        let previous_revision = next_revision
            .checked_sub(1)
            .context("identity authorization revision has no predecessor")?;
        let updated = tx.execute("UPDATE identity_meta SET authorization_revision=?1 WHERE singleton=1 AND authorization_revision=?2",params![next_revision,previous_revision])?;
        conflict(updated == 1)?;
        Ok(receipt(&stored, &record))
    }
    pub fn human_account(
        &self,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Option<HumanAccountV1>> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_identity(&tx)?;
        let value = json_row::<HumanAccountV1>(
            &tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?;
        self.commit_checked(tx)?;
        Ok(value)
    }
    pub fn identity_memberships(
        &self,
        principal: &PrincipalRefV1,
    ) -> anyhow::Result<Vec<MembershipV1>> {
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_identity(&tx)?;
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
        let key = resource.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_identity(&tx)?;
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
        let key = principal.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_identity(&tx)?;
        let account: HumanAccountV1 = json_row(
            &tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?
        .ok_or(IdentityFailureV1::IdentityRevoked)?;
        if !account.active {
            return Err(IdentityFailureV1::IdentityRevoked.into());
        }
        let membership: MembershipV1 = json_row(
            &tx,
            "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
            membership_id,
        )?
        .ok_or(IdentityFailureV1::ContextRevoked)?;
        invalid(membership.tenant_id == self.tenant_id)?;
        let authorization_revision = authorization_revision(&tx)?;
        let scope =
            crate::identity::authorize_context(principal, &membership, mode, expected_revision)?
                .at_authorization_revision(authorization_revision);
        self.commit_checked(tx)?;
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
        let resource_key = resource.key()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let current = revalidate_scope(&tx, scope, &self.tenant_id)?;
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
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_identity(&tx)?;
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
