//! Durable Review runner binding in the existing tenant event ledger. Modern
//! source/marker guards are mandatory, including before an idempotent lookup.
use super::*;
use crate::api::AuthenticatedReviewRunnerCredential;
use crate::control::review_binding::{self as contract, *};
use crate::human_authority::{
    HumanSourceKindV1, HumanSourcesReadEpoch, HumanSourcesWriteEpoch, OwnedSourceToken,
};
use crate::identity::{
    HumanAccountV1, HumanCapabilityV1, MembershipV1, ResourceGrantV1, ResourceKindV1,
    ResourcePermissionV1, ResourceRefV1, ResourceShareV1,
};
use contract::{conflict, permission, require};

pub(super) const SCHEMA: [(&str, &str, &str); 3] = [
    (
        "table",
        "runner_review_bindings",
        "CREATE TABLE runner_review_bindings(binding_id TEXT PRIMARY KEY NOT NULL,credential_id TEXT NOT NULL,tenant_id TEXT NOT NULL,active INTEGER NOT NULL CHECK(active IN (0,1)),binding_revision INTEGER NOT NULL CHECK(binding_revision>=1),policy_revision INTEGER NOT NULL CHECK(policy_revision>=1),snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL,last_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "runner_review_bindings_active_credential",
        "CREATE UNIQUE INDEX runner_review_bindings_active_credential ON runner_review_bindings(credential_id,tenant_id) WHERE active=1",
    ),
    (
        "index",
        "runner_review_bindings_active_tenant",
        "CREATE UNIQUE INDEX runner_review_bindings_active_tenant ON runner_review_bindings(tenant_id) WHERE active=1",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 1] = [("runner_review_bindings", 2)];
const PROVISION_KIND: &str = "control.review_binding.provisioned";
const REVOKE_KIND: &str = "control.review_binding.revoked";

fn require_modern(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if matches!(version, 14 | 15) {
        Ok(())
    } else {
        Err(ReviewBindingFailureV1::MigrationRequired.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
// Durable serde binding-change contract; preserve the value shape.
#[allow(clippy::large_enum_variant)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum BindingChangeV1 {
    Provision {
        command: ProvisionRunnerReviewBindingV1,
    },
    Revoke {
        command: RevokeRunnerReviewBindingV1,
    },
}
impl BindingChangeV1 {
    fn operation_id(&self) -> &str {
        match self {
            Self::Provision { command } => &command.operation_id,
            Self::Revoke { command } => &command.operation_id,
        }
    }
    fn binding_id(&self) -> &str {
        match self {
            Self::Provision { command } => &command.binding.binding_id,
            Self::Revoke { command } => &command.binding_id,
        }
    }
    fn expected_revision(&self) -> u64 {
        match self {
            Self::Provision { command } => command.expected_revision,
            Self::Revoke { command } => command.expected_revision,
        }
    }
    fn input_hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Provision { command } => command.input_hash(),
            Self::Revoke { command } => command.input_hash(),
        }
    }
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Provision { command } => command.validate(),
            Self::Revoke { command } => command.validate(),
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Provision { .. } => PROVISION_KIND,
            Self::Revoke { .. } => REVOKE_KIND,
        }
    }
}

/// Historical evidence, deliberately descriptive and never a live proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingRecordV1 {
    version: u32,
    authority_source_id: String,
    owner_incarnation: String,
    input_hash: String,
    previous_revision: u64,
    change: BindingChangeV1,
    snapshot: ReviewBindingSnapshotV1,
}
impl BindingRecordV1 {
    fn validate(&self, tenant: &str) -> anyhow::Result<()> {
        self.change.validate()?;
        self.snapshot.validate()?;
        require(
            self.version == 1
                && self.snapshot.binding.tenant_id == tenant
                && self.change.binding_id() == self.snapshot.binding.binding_id
                && self.input_hash == self.change.input_hash()?
                && self.previous_revision == self.change.expected_revision(),
        )?;
        for value in [&self.authority_source_id, &self.owner_incarnation] {
            require(
                uuid::Uuid::parse_str(value)
                    .is_ok_and(|id| !id.is_nil() && id.to_string() == *value),
            )?;
        }
        require(serde_json::to_vec(self)?.len() <= 120 * 1024)
    }
}

fn transition(
    previous: Option<&ReviewBindingSnapshotV1>,
    change: &BindingChangeV1,
) -> anyhow::Result<ReviewBindingSnapshotV1> {
    change.validate()?;
    conflict(previous.map_or(0, |old| old.binding.revision) == change.expected_revision())?;
    match change {
        BindingChangeV1::Provision { command } => {
            conflict(
                previous.map_or(0, |old| old.policy.revision) == command.expected_policy_revision,
            )?;
            if let Some(old) = previous {
                old.validate()?;
                // A technical credential is never reassigned to another tenant
                // or namespace. A revoked generation cannot be resurrected.
                conflict(
                    old.binding.credential_id == command.binding.credential_id
                        && old.binding.tenant_id == command.binding.tenant_id
                        && old.binding.control_namespace_uuid
                            == command.binding.control_namespace_uuid
                        && old.policy.policy_id == command.policy.policy_id,
                )?;
                conflict(
                    command.binding.credential_generation >= old.binding.credential_generation
                        && (old.binding.active
                            || command.binding.credential_generation
                                > old.binding.credential_generation),
                )?;
                conflict(
                    command.binding.credential_generation
                        <= old.binding.credential_generation.saturating_add(1),
                )?;
            }
            let snapshot = ReviewBindingSnapshotV1 {
                version: 1,
                binding: command.binding.clone(),
                policy: command.policy.clone(),
                binding_digest: command.binding.digest()?,
                policy_digest: command.policy.digest()?,
            };
            snapshot.validate()?;
            Ok(snapshot)
        }
        BindingChangeV1::Revoke { command } => {
            let old = previous.ok_or(ReviewBindingFailureV1::NotFound)?;
            old.validate()?;
            conflict(old.binding.active && old.binding_digest == command.expected_binding_digest)?;
            let mut snapshot = old.clone();
            snapshot.binding.revision = snapshot
                .binding
                .revision
                .checked_add(1)
                .ok_or(ReviewBindingFailureV1::Conflict)?;
            contract::revision(snapshot.binding.revision)?;
            snapshot.binding.active = false;
            snapshot.binding_digest = snapshot.binding.digest()?;
            snapshot.validate()?;
            Ok(snapshot)
        }
    }
}

#[derive(Default)]
struct Projection {
    snapshots: BTreeMap<String, (ReviewBindingSnapshotV1, i64)>,
    operations: BTreeMap<String, (String, i64)>,
    last_seq: i64,
}
fn ensure_cardinality(
    projection: &Projection,
    next: &ReviewBindingSnapshotV1,
) -> anyhow::Result<()> {
    if !next.binding.active {
        return Ok(());
    }
    for (id, (other, _)) in &projection.snapshots {
        if id == &next.binding.binding_id {
            continue;
        }
        conflict(
            !other.binding.active
                || (other.binding.tenant_id != next.binding.tenant_id
                    && other.binding.credential_id != next.binding.credential_id),
        )?;
        if other.binding.credential_id == next.binding.credential_id {
            // New binding IDs are not a route around the credential's durable
            // generation floor or its original tenant/namespace assignment.
            conflict(
                next.binding.tenant_id == other.binding.tenant_id
                    && next.binding.control_namespace_uuid == other.binding.control_namespace_uuid
                    && next.binding.credential_generation > other.binding.credential_generation,
            )?;
        }
        // A new binding ID must use its own policy identity, rather than reset
        // the revision of a previously provisioned logical policy to one.
        conflict(next.policy.policy_id != other.policy.policy_id)?;
    }
    Ok(())
}
impl Projection {
    fn observe(&mut self, stored: &StoredEvent, tenant: &str) -> anyhow::Result<()> {
        self.apply(&record(stored, tenant)?, stored.seq, tenant)
    }

    fn apply(&mut self, record: &BindingRecordV1, seq: i64, tenant: &str) -> anyhow::Result<()> {
        record.validate(tenant)?;
        require(seq > self.last_seq)?;
        conflict(!self.operations.contains_key(record.change.operation_id()))?;
        let previous = self.snapshots.get(record.change.binding_id());
        let next = transition(previous.map(|(snapshot, _)| snapshot), &record.change)?;
        require(next == record.snapshot)?;
        // The first vertical admits one active tuple per tenant. Credential
        // reuse across different sources is additionally rejected by Root's
        // whole-closure index loader; this projection cannot inspect another DB.
        ensure_cardinality(self, &next)?;
        self.operations.insert(
            record.change.operation_id().into(),
            (record.input_hash.clone(), seq),
        );
        self.snapshots
            .insert(next.binding.binding_id.clone(), (next, seq));
        self.last_seq = seq;
        Ok(())
    }

    fn rows(&self) -> impl Iterator<Item = anyhow::Result<ReviewBindingRow>> + '_ {
        self.snapshots
            .values()
            .map(|(snapshot, seq)| ReviewBindingRow::from_snapshot(snapshot, *seq))
    }
}

fn decode(event: &Event, tenant: &str) -> anyhow::Result<BindingRecordV1> {
    require(
        [PROVISION_KIND, REVOKE_KIND].contains(&event.kind.as_str())
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "native"
            && event.session_id.is_none()
            && event.project_id.is_none()
            && event.sandbox_id.is_none()
            && event.agent_id.is_none()
            && event.parent_agent_id.is_none()
            && event.correlation_id.is_none()
            && event.trace_id.is_none()
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none(),
    )?;
    require(serde_json::to_vec(&event.data)?.len() <= 128 * 1024)?;
    let record: BindingRecordV1 = serde_json::from_value(event.data.clone())
        .map_err(|_| ReviewBindingFailureV1::Integrity)?;
    record.validate(tenant)?;
    require(event.id == record.change.operation_id() && event.kind == record.change.kind())?;
    Ok(record)
}

/// Strict descriptive dispatch for the parent control replay. Recognizing this
/// closed marker never constructs a runner identity or source authority.
pub(super) fn marker(event: &Event, tenant: &str) -> anyhow::Result<()> {
    decode(event, tenant).map(|_| ())
}

fn record(stored: &StoredEvent, tenant: &str) -> anyhow::Result<BindingRecordV1> {
    decode(&stored.event, tenant)
}

type SqlRow = Vec<rusqlite::types::Value>;

/// The columns derived from the verified binding history before SQL encoding.
struct ReviewBindingRow {
    binding_id: String,
    credential_id: String,
    tenant_id: String,
    active: bool,
    binding_revision: i64,
    policy_revision: i64,
    snapshot_json: String,
    snapshot_hash: String,
    last_event_seq: i64,
}

impl ReviewBindingRow {
    fn from_snapshot(
        snapshot: &ReviewBindingSnapshotV1,
        last_event_seq: i64,
    ) -> anyhow::Result<Self> {
        snapshot.validate()?;
        Ok(Self {
            binding_id: snapshot.binding.binding_id.clone(),
            credential_id: snapshot.binding.credential_id.clone(),
            tenant_id: snapshot.binding.tenant_id.clone(),
            active: snapshot.binding.active,
            binding_revision: i64::try_from(snapshot.binding.revision)?,
            policy_revision: i64::try_from(snapshot.policy.revision)?,
            snapshot_json: serde_json::to_string(snapshot)?,
            snapshot_hash: snapshot.snapshot_hash()?,
            last_event_seq,
        })
    }

    fn into_sqlite_values(self) -> SqlRow {
        use rusqlite::types::Value as S;
        vec![
            S::Text(self.binding_id),
            S::Text(self.credential_id),
            S::Text(self.tenant_id),
            S::Integer(i64::from(self.active)),
            S::Integer(self.binding_revision),
            S::Integer(self.policy_revision),
            S::Text(self.snapshot_json),
            S::Text(self.snapshot_hash),
            S::Integer(self.last_event_seq),
        ]
    }
}
// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn load_snapshot(
    db: &Connection,
    binding_id: &str,
) -> anyhow::Result<Option<ReviewBindingSnapshotV1>> {
    let value = db
        .query_row(
            "SELECT snapshot_json FROM runner_review_bindings WHERE binding_id=?1",
            [binding_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|value| {
            require(value.len() <= 100 * 1024)?;
            let snapshot: ReviewBindingSnapshotV1 =
                serde_json::from_str(&value).map_err(|_| ReviewBindingFailureV1::Integrity)?;
            snapshot.validate()?;
            Ok(snapshot)
        })
        .transpose()
}
// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn write_snapshot(
    db: &Connection,
    snapshot: &ReviewBindingSnapshotV1,
    seq: i64,
    previous: Option<u64>,
) -> anyhow::Result<()> {
    let mut row = ReviewBindingRow::from_snapshot(snapshot, seq)?.into_sqlite_values();
    let changed = if let Some(previous) = previous {
        row.push(rusqlite::types::Value::Integer(i64::try_from(previous)?));
        db.execute("UPDATE runner_review_bindings SET credential_id=?2,tenant_id=?3,active=?4,binding_revision=?5,policy_revision=?6,snapshot_json=?7,snapshot_hash=?8,last_event_seq=?9 WHERE binding_id=?1 AND binding_revision=?10",rusqlite::params_from_iter(row))?
    } else {
        db.execute("INSERT INTO runner_review_bindings(binding_id,credential_id,tenant_id,active,binding_revision,policy_revision,snapshot_json,snapshot_hash,last_event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",rusqlite::params_from_iter(row))?
    };
    conflict(changed == 1)
}
fn verify_rows(db: &Connection, projection: &Projection) -> anyhow::Result<()> {
    let expected = projection
        .rows()
        .map(|row| row.map(ReviewBindingRow::into_sqlite_values))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut statement=db.prepare("SELECT binding_id,credential_id,tenant_id,active,binding_revision,policy_revision,snapshot_json,snapshot_hash,last_event_seq FROM runner_review_bindings ORDER BY binding_id")?;
    let actual = statement
        .query_map([], |row| {
            (0..9)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<SqlRow>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if expected == actual {
        Ok(())
    } else {
        Err(ReviewBindingFailureV1::Integrity.into())
    }
}

fn replay(db: &Connection, tenant: &str) -> anyhow::Result<Projection> {
    let mut statement=db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.review_binding.*' ORDER BY seq")?;
    let events = statement.query_map([], row_event)?;
    let mut projection = Projection::default();
    for stored in events {
        projection.observe(&stored?, tenant)?;
    }
    verify_rows(db, &projection)?;
    Ok(projection)
}
/// Called by the parent schema verifier. Replay is a descriptive integrity
/// check, not readiness or authority to accept modern writes.
pub(super) fn validate(db: &Connection, tenant: &str) -> anyhow::Result<()> {
    require_modern(db)?;
    replay(db, tenant).map(|_| ())
}

// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn receipt(stored: &StoredEvent, record: &BindingRecordV1) -> ReviewBindingReceiptV1 {
    ReviewBindingReceiptV1 {
        version: 1,
        operation_id: record.change.operation_id().into(),
        input_hash: record.input_hash.clone(),
        binding_id: record.snapshot.binding.binding_id.clone(),
        binding_revision: record.snapshot.binding.revision,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}

// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn historical_result(
    stored: &StoredEvent,
    change: &BindingChangeV1,
    tenant: &str,
    authority_source_id: &str,
) -> anyhow::Result<ReviewBindingMutationResultV1> {
    conflict(stored.event.kind == change.kind())?;
    let record = record(stored, tenant)?;
    conflict(
        record.change == *change
            && record.input_hash == change.input_hash()?
            && record.authority_source_id == authority_source_id,
    )?;
    Ok(ReviewBindingMutationResultV1 {
        version: 1,
        receipt: receipt(stored, &record),
        snapshot: record.snapshot,
        replayed: true,
    })
}

fn source_matches(
    store: &Store,
    source: &OwnedSourceToken,
    binding: &RunnerReviewBindingV1,
) -> anyhow::Result<()> {
    let descriptor = source.descriptor();
    let closure = source.closure();
    permission(
        descriptor.kind == HumanSourceKindV1::TenantStore
            && descriptor.tenant_id.as_deref() == Some(store.tenant_id())
            && binding.tenant_id == store.tenant_id()
            && descriptor.durable_namespace_id.to_string() == binding.control_namespace_uuid
            && matches!(descriptor.required_format, 14 | 15)
            && closure.closure_id.to_string() == binding.closure_id
            && closure.closure_revision == binding.closure_revision
            && closure.closure_digest == binding.closure_digest,
    )
}

fn validate_policy_accounts(
    db: &Connection,
    policy: &ReviewSessionBindingPolicyV1,
) -> anyhow::Result<()> {
    policy.validate()?;
    for (principal, approve) in std::iter::once((
        &policy.owner,
        policy
            .owner_permissions
            .contains(&ReviewSessionPermissionV1::ApproveLaunch),
    ))
    .chain(policy.shares.iter().map(|share| {
        (
            &share.principal,
            share
                .permissions
                .contains(&ReviewSessionPermissionV1::ApproveLaunch),
        )
    })) {
        let key = principal.key()?;
        let account: HumanAccountV1 = identity::json_row(
            db,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?
        .ok_or(ReviewBindingFailureV1::PermissionDenied)?;
        account.validate()?;
        permission(account.active && &account.principal == principal)?;
        let mut statement=db.prepare("SELECT payload_json FROM identity_memberships WHERE principal_key=?1 AND organization_scope=?2 ORDER BY membership_id")?;
        let memberships = statement
            .query_map(params![key, policy.organization_scope], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        permission(memberships.len() == 1)?;
        let membership: MembershipV1 =
            serde_json::from_str(&memberships[0]).map_err(|_| ReviewBindingFailureV1::Integrity)?;
        membership.validate()?;
        permission(
            membership.active
                && membership.principal == *principal
                && membership.tenant_id == policy.tenant_id
                && membership.organization_scope == policy.organization_scope
                && membership
                    .capabilities
                    .contains(&HumanCapabilityV1::ReadContent)
                && (!approve
                    || membership
                        .capabilities
                        .contains(&HumanCapabilityV1::ApproveLaunch)),
        )?;
    }
    Ok(())
}

// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn derive_session_grant(
    policy: &ReviewSessionBindingPolicyV1,
    session_id: &str,
) -> anyhow::Result<ResourceGrantV1> {
    policy.validate()?;
    require(
        uuid::Uuid::parse_str(session_id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == session_id),
    )?;
    permission(policy.active)?;
    let grant = ResourceGrantV1 {
        version: 1,
        tenant_id: policy.tenant_id.clone(),
        organization_scope: policy.organization_scope.clone(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: session_id.into(),
        },
        owner: policy.owner.clone(),
        revision: 1,
        active: true,
        shares: policy
            .shares
            .iter()
            .map(|share| ResourceShareV1 {
                principal: share.principal.clone(),
                permissions: share
                    .permissions
                    .iter()
                    .filter_map(|permission| match permission {
                        ReviewSessionPermissionV1::Content => Some(ResourcePermissionV1::Content),
                        // The explicit approval ACL belongs to the registered
                        // session proof, never the legacy grant's owner shortcut.
                        ReviewSessionPermissionV1::ApproveLaunch => None,
                    })
                    .collect(),
            })
            .collect(),
    };
    grant.validate()?;
    Ok(grant)
}

// Closed Review binding mutation target; production wiring pending.
#[allow(dead_code)]
impl Store {
    /// Operator maintenance only. The owned write epoch, real modern source
    /// marker and private credential configuration are supplied by trusted glue.
    pub(crate) fn provision_review_binding(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        command: &ProvisionRunnerReviewBindingV1,
    ) -> anyhow::Result<ReviewBindingMutationResultV1> {
        self.mutate_review_binding(
            epoch,
            source,
            BindingChangeV1::Provision {
                command: command.clone(),
            },
        )
    }
    pub(crate) fn revoke_review_binding(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        command: &RevokeRunnerReviewBindingV1,
    ) -> anyhow::Result<ReviewBindingMutationResultV1> {
        self.mutate_review_binding(
            epoch,
            source,
            BindingChangeV1::Revoke {
                command: command.clone(),
            },
        )
    }
    fn mutate_review_binding(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        change: BindingChangeV1,
    ) -> anyhow::Result<ReviewBindingMutationResultV1> {
        change.validate()?;
        epoch.validate_source(source)?;
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| ReviewBindingFailureV1::Unavailable)?;
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        tx.require_identity_authority()?;
        require_modern(&tx)?;
        self.revalidate_review_binding_source_in_transaction(&tx, source)?;
        let clock = workload::capture_control_time(&tx)?;
        let projection = replay(&tx, self.tenant_id())?;
        let previous = projection
            .snapshots
            .get(change.binding_id())
            .map(|(snapshot, _)| snapshot);
        let basis = match &change {
            BindingChangeV1::Provision { command } => &command.binding,
            BindingChangeV1::Revoke { .. } => {
                &previous.ok_or(ReviewBindingFailureV1::NotFound)?.binding
            }
        };
        source_matches(self, source, basis)?;
        let old = tx
            .query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                [change.operation_id()],
                row_event,
            )
            .optional()?;
        if let Some(stored) = old {
            let result = historical_result(
                &stored,
                &change,
                self.tenant_id(),
                &source.source_id().to_string(),
            )?;
            epoch.validate_source(source)?;
            self.revalidate_review_binding_source_in_transaction(&tx, source)?;
            self.commit_authorized(tx, &clock)?;
            epoch.validate_source(source)?;
            return Ok(result);
        }
        if let BindingChangeV1::Provision { command } = &change {
            validate_policy_accounts(&tx, &command.policy)?;
        }
        let snapshot = transition(previous, &change)?;
        ensure_cardinality(&projection, &snapshot)?;
        let record = BindingRecordV1 {
            version: 1,
            authority_source_id: source.source_id().to_string(),
            owner_incarnation: source.incarnation().to_string(),
            input_hash: change.input_hash()?,
            previous_revision: change.expected_revision(),
            change,
            snapshot,
        };
        record.validate(self.tenant_id())?;
        let mut event = Event::new(record.change.kind());
        event.id = record.change.operation_id().into();
        event.timestamp = crate::broker_registry::format_time(clock.at());
        event.coverage = "native".into();
        event.data = serde_json::to_value(&record)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        permission(serde_json::to_value(&event)? == original)?;
        let stored = self.append_authorized_identity_in_transaction(&tx, event, payload, &clock)?;
        write_snapshot(
            &tx,
            &record.snapshot,
            stored.seq,
            previous.map(|old| old.binding.revision),
        )?;
        // Check the newly persisted projection against actual event replay
        // before committing, rather than trusting the supplied snapshot.
        replay(&tx, self.tenant_id())?;
        epoch.validate_source(source)?;
        self.revalidate_review_binding_source_in_transaction(&tx, source)?;
        self.commit_authorized(tx, &clock)?;
        epoch.validate_source(source)?;
        Ok(ReviewBindingMutationResultV1 {
            version: 1,
            receipt: receipt(&stored, &record),
            snapshot: record.snapshot,
            replayed: false,
        })
    }
}

#[derive(Clone, Copy)]
struct CredentialTuple<'a> {
    tenant_id: &'a str,
    credential_id: &'a str,
    credential_generation: u64,
    closure_id: &'a str,
    closure_revision: u64,
    control_namespace_uuid: &'a str,
    config_public_digest: &'a str,
}
fn compare_authenticated_tuple(
    binding: &RunnerReviewBindingV1,
    credential: &CredentialTuple<'_>,
) -> anyhow::Result<()> {
    binding.validate()?;
    permission(
        binding.active
            && binding.tenant_id == credential.tenant_id
            && binding.credential_id == credential.credential_id
            && binding.credential_generation == credential.credential_generation
            && binding.closure_id == credential.closure_id
            && binding.closure_revision == credential.closure_revision
            && binding.control_namespace_uuid == credential.control_namespace_uuid
            && binding.control_config_public_digest == credential.config_public_digest,
    )
}
fn compare_credential(
    binding: &RunnerReviewBindingV1,
    credential: &AuthenticatedReviewRunnerCredential,
) -> anyhow::Result<()> {
    credential.revalidate()?;
    compare_authenticated_tuple(
        binding,
        &CredentialTuple {
            tenant_id: credential.tenant_id(),
            credential_id: &credential.credential_id().to_string(),
            credential_generation: credential.credential_generation(),
            closure_id: &credential.closure_id().to_string(),
            closure_revision: credential.closure_revision(),
            control_namespace_uuid: &credential.control_namespace_uuid().to_string(),
            config_public_digest: credential.config_public_digest(),
        },
    )
}

// Closed Review binding ledger target; production wiring pending.
#[allow(dead_code)]
fn compare_retained_binding(
    current: &ReviewBindingSnapshotV1,
    retained: &RunnerReviewBindingV1,
    retained_digest: &str,
) -> anyhow::Result<()> {
    current.validate()?;
    retained.validate()?;
    permission(current.binding == *retained && current.binding_digest == retained_digest)
}

enum BindingEpoch<'a> {
    Read(&'a HumanSourcesReadEpoch),
    Write(&'a HumanSourcesWriteEpoch),
}
impl BindingEpoch<'_> {
    fn validate_source(&self, source: &OwnedSourceToken) -> anyhow::Result<()> {
        match self {
            Self::Read(epoch) => epoch.validate_source(source)?,
            Self::Write(epoch) => epoch.validate_source(source)?,
        };
        Ok(())
    }
}

/// Transaction-bound live check, never Serialize/Deserialize/Clone. It is not
/// a human identity or a launch approval. A public snapshot cannot recreate it.
pub(crate) struct VerifiedRunnerReviewBinding<'tx, 'connection> {
    store: &'tx Store,
    tx: &'tx CheckedTransaction<'connection>,
    epoch: BindingEpoch<'tx>,
    source: &'tx OwnedSourceToken,
    credential: &'tx AuthenticatedReviewRunnerCredential,
    snapshot: ReviewBindingSnapshotV1,
}
impl VerifiedRunnerReviewBinding<'_, '_> {
    pub(crate) fn binding(&self) -> &RunnerReviewBindingV1 {
        &self.snapshot.binding
    }
    // Closed Review binding read target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn policy(&self) -> &ReviewSessionBindingPolicyV1 {
        &self.snapshot.policy
    }
    pub(crate) fn binding_digest(&self) -> &str {
        &self.snapshot.binding_digest
    }
    // Closed Review binding read target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn snapshot(&self) -> &ReviewBindingSnapshotV1 {
        &self.snapshot
    }
    /// Consent, admission and claim compare the exact server tuple retained by
    /// their target; no revision, generation, routing or project remapping.
    // Closed Review binding read target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn require_exact_binding(
        &self,
        retained: &RunnerReviewBindingV1,
        retained_digest: &str,
    ) -> anyhow::Result<()> {
        self.revalidate()?;
        compare_retained_binding(&self.snapshot, retained, retained_digest)
    }
    /// Re-read before the caller's final commit; keeping this proof does not
    /// excuse the caller from its own human-source and target checks.
    pub(crate) fn revalidate(&self) -> anyhow::Result<()> {
        match &self.epoch {
            BindingEpoch::Read(_) => self.tx.require_business_access()?,
            BindingEpoch::Write(_) => self.tx.require_identity_authority()?,
        }
        self.epoch.validate_source(self.source)?;
        self.store
            .revalidate_review_binding_source_in_transaction(self.tx, self.source)?;
        source_matches(self.store, self.source, &self.snapshot.binding)?;
        compare_credential(&self.snapshot.binding, self.credential)?;
        let projection = replay(self.tx, self.store.tenant_id())?;
        let (current, _) = projection
            .snapshots
            .get(&self.snapshot.binding.binding_id)
            .ok_or(ReviewBindingFailureV1::PermissionDenied)?;
        permission(
            *current == self.snapshot
                && current.binding.active
                && current.policy.active
                && projection
                    .snapshots
                    .values()
                    .filter(|(snapshot, _)| snapshot.binding.active)
                    .count()
                    == 1,
        )?;
        validate_policy_accounts(self.tx, &current.policy)
    }
    /// Only policy data selects owner/shares. There is no runner-supplied
    /// principal, organization, project, share or permission argument.
    /// This legacy Content grant is not proof of an approval ACL or consent.
    // Closed Review binding read target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn session_grant(&self, session_id: &str) -> anyhow::Result<ResourceGrantV1> {
        self.revalidate()?;
        derive_session_grant(&self.snapshot.policy, session_id)
    }
}

impl Store {
    pub(crate) fn verify_review_binding_in_transaction<'tx, 'connection>(
        &'tx self,
        tx: &'tx CheckedTransaction<'connection>,
        epoch: &'tx HumanSourcesReadEpoch,
        source: &'tx OwnedSourceToken,
        credential: &'tx AuthenticatedReviewRunnerCredential,
    ) -> anyhow::Result<VerifiedRunnerReviewBinding<'tx, 'connection>> {
        tx.require_business_access()?;
        self.verify_review_binding(tx, BindingEpoch::Read(epoch), source, credential)
    }
    pub(crate) fn verify_review_binding_for_registration_in_transaction<'tx, 'connection>(
        &'tx self,
        tx: &'tx CheckedTransaction<'connection>,
        epoch: &'tx HumanSourcesWriteEpoch,
        source: &'tx OwnedSourceToken,
        credential: &'tx AuthenticatedReviewRunnerCredential,
    ) -> anyhow::Result<VerifiedRunnerReviewBinding<'tx, 'connection>> {
        tx.require_identity_authority()?;
        self.verify_review_binding(tx, BindingEpoch::Write(epoch), source, credential)
    }
    fn verify_review_binding<'tx, 'connection>(
        &'tx self,
        tx: &'tx CheckedTransaction<'connection>,
        epoch: BindingEpoch<'tx>,
        source: &'tx OwnedSourceToken,
        credential: &'tx AuthenticatedReviewRunnerCredential,
    ) -> anyhow::Result<VerifiedRunnerReviewBinding<'tx, 'connection>> {
        match &epoch {
            BindingEpoch::Read(_) => tx.require_business_access()?,
            BindingEpoch::Write(_) => tx.require_identity_authority()?,
        }
        epoch.validate_source(source)?;
        require_modern(tx)?;
        self.revalidate_review_binding_source_in_transaction(tx, source)?;
        permission(credential.tenant_id() == self.tenant_id())?;
        let projection = replay(tx, self.tenant_id())?;
        let mut active = projection
            .snapshots
            .values()
            .filter(|(snapshot, _)| snapshot.binding.active);
        let snapshot = active
            .next()
            .ok_or(ReviewBindingFailureV1::PermissionDenied)?
            .0
            .clone();
        permission(active.next().is_none())?;
        source_matches(self, source, &snapshot.binding)?;
        compare_credential(&snapshot.binding, credential)?;
        permission(snapshot.policy.active)?;
        validate_policy_accounts(tx, &snapshot.policy)?;
        let verified = VerifiedRunnerReviewBinding {
            store: self,
            tx,
            epoch,
            source,
            credential,
            snapshot,
        };
        verified.revalidate()?;
        Ok(verified)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[test]
fn sqlite_projection_refuses_a_different_event_sequence_for_the_same_snapshot() -> anyhow::Result<()>
{
    let db = Connection::open_in_memory()?;
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE events(seq INTEGER PRIMARY KEY);")?;
    for (_, _, sql) in SCHEMA {
        db.execute_batch(sql)?;
    }
    db.execute("INSERT INTO events(seq) VALUES(1),(2)", [])?;

    let command = crate::control::review_binding::tests::provision();
    let change = BindingChangeV1::Provision { command };
    let snapshot = transition(None, &change)?;
    let record = BindingRecordV1 {
        version: 1,
        authority_source_id: "11000000-0000-4000-8000-000000000020".into(),
        owner_incarnation: "11000000-0000-4000-8000-000000000021".into(),
        input_hash: change.input_hash()?,
        previous_revision: change.expected_revision(),
        change,
        snapshot,
    };
    let mut event = Event::new(record.change.kind());
    event.id = record.change.operation_id().into();
    event.source = "gateway".into();
    event.provenance = "observed".into();
    event.coverage = "native".into();
    event.data = serde_json::to_value(&record)?;
    let stored = StoredEvent {
        seq: 2,
        received_at: event.timestamp.clone(),
        prev_hash: "a".repeat(64),
        hash: "b".repeat(64),
        event,
    };
    let mut projection = Projection::default();
    projection.observe(&stored, &record.snapshot.binding.tenant_id)?;
    write_snapshot(&db, &record.snapshot, stored.seq, None)?;
    verify_rows(&db, &projection)?;

    // The foreign key still refers to an existing event. All other columns match.
    assert_eq!(
        db.execute("UPDATE runner_review_bindings SET last_event_seq=1", [])?,
        1
    );
    let error = verify_rows(&db, &projection).expect_err("the last event must match the replay");
    assert_eq!(
        error.downcast_ref::<ReviewBindingFailureV1>(),
        Some(&ReviewBindingFailureV1::Integrity)
    );
    Ok(())
}
