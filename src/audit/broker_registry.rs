//! Broker projections are reconstructed from typed, durable control events.
use super::*;
use crate::broker_registry::*;
use chrono::{DateTime, Duration, Utc};

pub(super) const SCHEMA: [(&str, &str, &str); 6] = [
    (
        "table",
        "broker_registry_meta",
        "CREATE TABLE broker_registry_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "broker_registry_brokers",
        "CREATE TABLE broker_registry_brokers(broker_id TEXT PRIMARY KEY,grant_revision INTEGER NOT NULL,heartbeat_revision INTEGER NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "table",
        "broker_registry_authorities",
        "CREATE TABLE broker_registry_authorities(authority_id TEXT PRIMARY KEY,broker_id TEXT NOT NULL REFERENCES broker_registry_brokers(broker_id),runtime_kind TEXT NOT NULL)",
    ),
    (
        "table",
        "broker_registry_incarnations",
        "CREATE TABLE broker_registry_incarnations(incarnation_id TEXT PRIMARY KEY,broker_id TEXT NOT NULL REFERENCES broker_registry_brokers(broker_id),event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "broker_registry_operations",
        "CREATE TABLE broker_registry_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),broker_id TEXT NOT NULL REFERENCES broker_registry_brokers(broker_id),result_json TEXT NOT NULL)",
    ),
    (
        "index",
        "broker_registry_operations_prefix",
        "CREATE INDEX broker_registry_operations_prefix ON broker_registry_operations(broker_id,event_seq)",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 5] = [
    ("broker_registry_meta", 1),
    ("broker_registry_brokers", 1),
    ("broker_registry_authorities", 1),
    ("broker_registry_incarnations", 2),
    ("broker_registry_operations", 2),
];
const QUERIES: [(&str, &str); 5] = [
    (
        "broker_registry_meta",
        "SELECT singleton,genesis_seq,genesis_hash FROM broker_registry_meta ORDER BY singleton",
    ),
    (
        "broker_registry_brokers",
        "SELECT broker_id,grant_revision,heartbeat_revision,payload_json,event_seq FROM broker_registry_brokers ORDER BY broker_id",
    ),
    (
        "broker_registry_authorities",
        "SELECT authority_id,broker_id,runtime_kind FROM broker_registry_authorities ORDER BY authority_id",
    ),
    (
        "broker_registry_incarnations",
        "SELECT incarnation_id,broker_id,event_seq FROM broker_registry_incarnations ORDER BY incarnation_id",
    ),
    (
        "broker_registry_operations",
        "SELECT operation_id,input_hash,event_seq,broker_id,result_json FROM broker_registry_operations ORDER BY operation_id",
    ),
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    tenant_id: String,
    control_namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}

// Journal evidence is private to this module. Deserializing it is never a
// current authentication path: replay validates the historical binding too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityEvidence {
    tenant_id: String,
    broker_id: String,
    credential_id: String,
    binding_revision: u64,
    grant_revision: u64,
}
impl IdentityEvidence {
    fn capture(identity: &BrokerIdentityV1) -> Self {
        Self {
            tenant_id: identity.tenant_id().into(),
            broker_id: identity.broker_id().into(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        }
    }
    fn identity(&self) -> anyhow::Result<BrokerIdentityV1> {
        BrokerIdentityV1::authenticated(
            self.tenant_id.clone(),
            self.broker_id.clone(),
            self.credential_id.clone(),
            self.binding_revision,
            self.grant_revision,
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Grant {
        mutation: BrokerGrantMutationV1,
    },
    Revoke {
        mutation: BrokerRevocationV1,
    },
    Heartbeat {
        identity: IdentityEvidence,
        heartbeat: BrokerHeartbeatV1,
    },
}
impl Change {
    fn operation_id(&self) -> &str {
        match self {
            Self::Grant { mutation } => &mutation.operation_id,
            Self::Revoke { mutation } => &mutation.operation_id,
            Self::Heartbeat { heartbeat, .. } => &heartbeat.operation_id,
        }
    }
    fn broker_id(&self) -> &str {
        match self {
            Self::Grant { mutation } => &mutation.broker_id,
            Self::Revoke { mutation } => &mutation.broker_id,
            Self::Heartbeat { identity, .. } => &identity.broker_id,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Grant { .. } => "control.broker_registry.grant",
            Self::Revoke { .. } => "control.broker_registry.revoke",
            Self::Heartbeat { .. } => "control.broker_registry.heartbeat",
        }
    }
    fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::content_hash("ai-guard:broker-registry-input:v1", self)
    }
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Grant { mutation } => mutation.validate(),
            Self::Revoke { mutation } => mutation.validate(),
            Self::Heartbeat {
                identity,
                heartbeat,
            } => {
                identity.identity()?;
                heartbeat.validate()
            }
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    input_hash: String,
    /// Captured once under the control transaction, never supplied by broker.
    received_at: String,
    change: Change,
}

pub(super) fn workload_audit_identity(
    event: &Event,
) -> anyhow::Result<Option<super::capacity::WorkloadAuditPrincipalV1>> {
    if !event.kind.starts_with("control.broker_registry.")
        || event.kind == "control.broker_registry.genesis"
    {
        return Ok(None);
    }
    let record: Record = serde_json::from_value(event.data.clone())?;
    record.change.validate()?;
    require(
        record.version == 1
            && record.input_hash == record.change.input_hash()?
            && event.kind == record.change.kind()
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "observed",
    )?;
    match record.change {
        Change::Heartbeat { identity, .. } => {
            Ok(Some(super::capacity::WorkloadAuditPrincipalV1::Broker {
                broker_id: identity.broker_id,
            }))
        }
        Change::Grant { .. } | Change::Revoke { .. } => Ok(None),
    }
}

#[derive(Default)]
struct Projection {
    brokers: BTreeMap<String, (BrokerV1, i64)>,
    authorities: BTreeMap<String, (String, BrokerRuntimeKindV1)>,
    incarnations: BTreeMap<String, (String, i64)>,
    operations: BTreeMap<String, (String, i64, BrokerV1)>,
}

fn permission(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::PermissionDenied.into())
    }
}
fn clock(previous: &BrokerV1, now: DateTime<Utc>) -> anyhow::Result<()> {
    if now >= previous.latest_received_at()? {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::ClockRegression.into())
    }
}

impl Projection {
    fn apply(&mut self, record: &Record, tenant: &str, seq: i64) -> anyhow::Result<BrokerV1> {
        record.change.validate()?;
        require(record.version == 1 && record.input_hash == record.change.input_hash()?)?;
        conflict(!self.operations.contains_key(record.change.operation_id()))?;
        let now = parse_time(&record.received_at)?;
        let previous = self
            .brokers
            .get(record.change.broker_id())
            .map(|(broker, _)| broker);
        if let Some(previous) = previous {
            clock(previous, now)?;
        }
        let broker = match &record.change {
            Change::Grant { mutation } => {
                conflict(
                    mutation.expected_revision == previous.map(|broker| broker.grant_revision),
                )?;
                let not_after =
                    super::workload::canonical_control_time(parse_time(&mutation.not_after)?)?;
                require(not_after > now)?;
                conflict(!self.incarnations.contains_key(&mutation.incarnation_id))?;
                for authority in &mutation.authorities {
                    if let Some((owner, kind)) = self.authorities.get(&authority.authority_id) {
                        conflict(owner == &mutation.broker_id && kind == &authority.runtime_kind)?;
                    }
                }
                let broker = BrokerV1 {
                    version: 1,
                    broker_id: mutation.broker_id.clone(),
                    grant_revision: next_revision(
                        previous.map_or(0, |broker| broker.grant_revision),
                    )?,
                    heartbeat_revision: previous.map_or(0, |broker| broker.heartbeat_revision),
                    credential_id: mutation.credential_id.clone(),
                    incarnation_id: mutation.incarnation_id.clone(),
                    authorities: mutation.authorities.clone(),
                    lease_seconds: mutation.lease_seconds,
                    not_after: format_time(not_after),
                    revoked: false,
                    granted_at: record.received_at.clone(),
                    revoked_at: None,
                    last_heartbeat: previous.and_then(|broker| broker.last_heartbeat.clone()),
                };
                for authority in &mutation.authorities {
                    self.authorities.insert(
                        authority.authority_id.clone(),
                        (broker.broker_id.clone(), authority.runtime_kind),
                    );
                }
                self.incarnations.insert(
                    mutation.incarnation_id.clone(),
                    (broker.broker_id.clone(), seq),
                );
                broker
            }
            Change::Revoke { mutation } => {
                let mut broker = previous.cloned().ok_or(BrokerRegistryFailureV1::NotFound)?;
                conflict(mutation.expected_revision == broker.grant_revision && !broker.revoked)?;
                broker.grant_revision = next_revision(broker.grant_revision)?;
                broker.revoked = true;
                broker.revoked_at = Some(record.received_at.clone());
                broker
            }
            Change::Heartbeat {
                identity,
                heartbeat,
            } => {
                permission(identity.tenant_id == tenant)?;
                let mut broker = previous
                    .cloned()
                    .ok_or(BrokerRegistryFailureV1::PermissionDenied)?;
                broker.require_current_grant(&identity.identity()?, &record.received_at)?;
                conflict(
                    heartbeat.incarnation_id == broker.incarnation_id
                        && heartbeat.expected_heartbeat_revision == broker.heartbeat_revision,
                )?;
                let previous_sequence = broker
                    .last_heartbeat
                    .as_ref()
                    .filter(|last| last.incarnation_id == broker.incarnation_id)
                    .map_or(0, |last| last.sequence);
                conflict(previous_sequence.checked_add(1) == Some(heartbeat.sequence))?;
                let lease_expires_at = now
                    .checked_add_signed(Duration::seconds(i64::from(broker.lease_seconds)))
                    .context("broker lease expiry overflow")?
                    .min(parse_time(&broker.not_after)?);
                broker.heartbeat_revision = next_revision(broker.heartbeat_revision)?;
                broker.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
                    incarnation_id: broker.incarnation_id.clone(),
                    sequence: heartbeat.sequence,
                    received_at: record.received_at.clone(),
                    lease_expires_at: format_time(lease_expires_at),
                    heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
                    reception_provenance: BrokerReceptionProvenanceV1::Observed,
                });
                broker
            }
        };
        self.brokers
            .insert(broker.broker_id.clone(), (broker.clone(), seq));
        self.operations.insert(
            record.change.operation_id().into(),
            (record.input_hash.clone(), seq, broker.clone()),
        );
        Ok(broker)
    }
}

fn require_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [6, 7].contains(&version) {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::MigrationRequired.into())
    }
}

pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut event = Event::new("control.broker_registry.genesis");
    event.data = serde_json::to_value(Genesis {
        version: 1,
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        previous_seq,
        previous_hash,
    })?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    require(serde_json::to_value(&event)? == original)?;
    let stored = store.append_in_transaction(tx, event, payload)?;
    tx.execute(
        "INSERT INTO broker_registry_meta(singleton,genesis_seq,genesis_hash) VALUES(1,?1,?2)",
        params![stored.seq, stored.hash],
    )?;
    Ok(())
}

type SqlRow = Vec<rusqlite::types::Value>;
fn text_value(value: impl Into<String>) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.into())
}
fn int_value(value: u64) -> anyhow::Result<rusqlite::types::Value> {
    Ok(rusqlite::types::Value::Integer(i64::try_from(value)?))
}
fn broker_row(broker: &BrokerV1, seq: i64) -> anyhow::Result<SqlRow> {
    Ok(vec![
        text_value(&broker.broker_id),
        int_value(broker.grant_revision)?,
        int_value(broker.heartbeat_revision)?,
        text_value(serde_json::to_string(broker)?),
        rusqlite::types::Value::Integer(seq),
    ])
}
fn runtime_name(kind: BrokerRuntimeKindV1) -> anyhow::Result<String> {
    Ok(serde_json::to_value(kind)?
        .as_str()
        .context("invalid broker runtime kind")?
        .into())
}

pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if ![6, 7].contains(&version) {
        return Ok(());
    }
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let (genesis_seq, genesis_hash): (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM broker_registry_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut projection = Projection::default();
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.broker_registry.*' ORDER BY seq")?;
    let mut seen_genesis = false;
    for stored in statement.query_map([], row_event)? {
        let stored = stored?;
        require(
            stored.event.source == "gateway"
                && stored.event.provenance == "observed"
                && stored.event.coverage == "observed",
        )?;
        if stored.event.kind == "control.broker_registry.genesis" {
            let genesis: Genesis = serde_json::from_value(stored.event.data.clone())?;
            require(
                !seen_genesis
                    && stored.seq == genesis_seq
                    && stored.hash == genesis_hash
                    && genesis.version == 1
                    && genesis.tenant_id == tenant
                    && genesis.control_namespace_uuid == namespace
                    && genesis.previous_seq == stored.seq - 1
                    && genesis.previous_hash == stored.prev_hash,
            )?;
            seen_genesis = true;
            continue;
        }
        require(seen_genesis)?;
        let record: Record = serde_json::from_value(stored.event.data.clone())?;
        require(
            stored.event.kind == record.change.kind()
                && parse_time(&record.received_at)? <= parse_time(&stored.received_at)?,
        )?;
        super::workload::validate_control_time(db, &record.received_at, Some(stored.seq))?;
        if let Change::Heartbeat { identity, .. } = &record.change {
            super::workload::validate_broker_identity_at(
                db,
                &identity.identity()?,
                parse_time(&record.received_at)?,
                stored.seq,
            )?;
        }
        projection.apply(&record, &tenant, stored.seq)?;
    }
    require(seen_genesis)?;
    let mut expected: BTreeMap<&str, Vec<SqlRow>> = BTreeMap::new();
    expected.insert(
        "broker_registry_meta",
        vec![vec![
            rusqlite::types::Value::Integer(1),
            rusqlite::types::Value::Integer(genesis_seq),
            text_value(genesis_hash),
        ]],
    );
    expected.insert(
        "broker_registry_brokers",
        projection
            .brokers
            .values()
            .map(|(broker, seq)| broker_row(broker, *seq))
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    expected.insert(
        "broker_registry_authorities",
        projection
            .authorities
            .iter()
            .map(|(id, (owner, kind))| {
                Ok(vec![
                    text_value(id),
                    text_value(owner),
                    text_value(runtime_name(*kind)?),
                ])
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    expected.insert(
        "broker_registry_incarnations",
        projection
            .incarnations
            .iter()
            .map(|(id, (owner, seq))| {
                vec![
                    text_value(id),
                    text_value(owner),
                    rusqlite::types::Value::Integer(*seq),
                ]
            })
            .collect(),
    );
    expected.insert(
        "broker_registry_operations",
        projection
            .operations
            .iter()
            .map(|(id, (hash, seq, broker))| {
                Ok(vec![
                    text_value(id),
                    text_value(hash),
                    rusqlite::types::Value::Integer(*seq),
                    text_value(&broker.broker_id),
                    text_value(serde_json::to_string(broker)?),
                ])
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    for (table, query) in QUERIES {
        let mut statement = db.prepare(query)?;
        let width = statement.column_count();
        let actual = statement
            .query_map([], |row| {
                (0..width)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<rusqlite::Result<SqlRow>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        anyhow::ensure!(
            expected.remove(table) == Some(actual),
            "broker registry projection differs from audit: {table}"
        );
    }
    Ok(())
}

fn load_broker(db: &Connection, id: &str) -> anyhow::Result<Option<BrokerV1>> {
    db.query_row(
        "SELECT payload_json FROM broker_registry_brokers WHERE broker_id=?1",
        [id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}

pub(super) fn broker_at(
    db: &Connection,
    id: &str,
    before_seq: i64,
) -> anyhow::Result<Option<BrokerV1>> {
    db.query_row("SELECT result_json FROM broker_registry_operations WHERE broker_id=?1 AND event_seq<?2 ORDER BY event_seq DESC LIMIT 1", params![id,before_seq], |row| row.get::<_,String>(0))
        .optional()?.map(|json| serde_json::from_str(&json).map_err(Into::into)).transpose()
}
fn preflight(db: &Connection, change: &Change) -> anyhow::Result<Projection> {
    let mut projection = Projection::default();
    if let Some(broker) = load_broker(db, change.broker_id())? {
        projection
            .brokers
            .insert(broker.broker_id.clone(), (broker, 0));
    }
    if let Change::Grant { mutation } = change {
        if let Some((broker_id,seq)) = db.query_row("SELECT broker_id,event_seq FROM broker_registry_incarnations WHERE incarnation_id=?1", [&mutation.incarnation_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).optional()? {
            projection.incarnations.insert(mutation.incarnation_id.clone(), (broker_id,seq));
        }
        for authority in &mutation.authorities {
            if let Some((broker_id,kind)) = db.query_row("SELECT broker_id,runtime_kind FROM broker_registry_authorities WHERE authority_id=?1", [&authority.authority_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()? {
                projection.authorities.insert(authority.authority_id.clone(), (broker_id,serde_json::from_value(json!(kind))?));
            }
        }
    }
    Ok(projection)
}

fn result(
    stored: &StoredEvent,
    broker: BrokerV1,
    input_hash: String,
    operation_id: String,
    replayed: bool,
) -> BrokerMutationResultV1 {
    BrokerMutationResultV1 {
        receipt: BrokerMutationReceiptV1 {
            operation_id,
            broker_id: broker.broker_id.clone(),
            grant_revision: broker.grant_revision,
            heartbeat_revision: broker.heartbeat_revision,
            input_hash,
            audit_seq: stored.seq,
            audit_hash: stored.hash.clone(),
        },
        broker,
        replayed,
    }
}
fn replay(db: &Connection, change: &Change) -> anyhow::Result<Option<BrokerMutationResultV1>> {
    let row = db.query_row("SELECT input_hash,event_seq,result_json FROM broker_registry_operations WHERE operation_id=?1", [change.operation_id()], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?))).optional()?;
    let Some((hash, seq, json)) = row else {
        return Ok(None);
    };
    conflict(hash == change.input_hash()?)?;
    let stored = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [seq],
        row_event,
    )?;
    conflict(stored.event.kind == change.kind())?;
    Ok(Some(result(
        &stored,
        serde_json::from_str(&json)?,
        hash,
        change.operation_id().into(),
        true,
    )))
}

impl Store {
    /// Compute the existing heartbeat commitment from checked DTO evidence.
    /// This does not authenticate the DTO, open a Store, or construct a current
    /// BrokerIdentityV1 proof. The original private Change serialization is
    /// deliberately shared with persistence, including its historical order.
    pub(crate) fn broker_heartbeat_input_hash(
        view: &BrokerIdentityViewV1,
        heartbeat: &BrokerHeartbeatV1,
    ) -> anyhow::Result<String> {
        require(view.version == 1 && matches!(view.schema_version, 6 | 7))?;
        crate::tenancy::validate_tenant_id(&view.tenant_id)?;
        for id in [
            &view.control_namespace_uuid,
            &view.broker_id,
            &view.credential_id,
            &view.incarnation_id,
        ] {
            registry_uuid(id)?;
        }
        require(view.binding_revision > 0 && view.grant_revision > 0)?;
        revision(view.binding_revision)?;
        revision(view.grant_revision)?;
        heartbeat.validate()?;
        require(heartbeat.incarnation_id == view.incarnation_id)?;
        Change::Heartbeat {
            identity: IdentityEvidence {
                tenant_id: view.tenant_id.clone(),
                broker_id: view.broker_id.clone(),
                credential_id: view.credential_id.clone(),
                binding_revision: view.binding_revision,
                grant_revision: view.grant_revision,
            },
            heartbeat: heartbeat.clone(),
        }
        .input_hash()
    }

    /// Trusted operator entry point. HTTP callers must already hold control
    /// mutation permission; this DTO never authenticates its broker subject.
    pub fn mutate_broker_grant(
        &self,
        mutation: &BrokerGrantMutationV1,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        mutation.validate()?;
        self.persist_broker_change(
            Change::Grant {
                mutation: mutation.clone(),
            },
            None,
        )
    }
    /// Revocation uses the private identity-maintenance capacity lane. The
    /// caller cannot select this lane through a kind, role string or heartbeat.
    pub fn revoke_broker(
        &self,
        mutation: &BrokerRevocationV1,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        mutation.validate()?;
        self.persist_broker_change(
            Change::Revoke {
                mutation: mutation.clone(),
            },
            None,
        )
    }
    pub fn broker_heartbeat(
        &self,
        identity: &BrokerIdentityV1,
        heartbeat: &BrokerHeartbeatV1,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        heartbeat.validate()?;
        self.persist_broker_change(
            Change::Heartbeat {
                identity: IdentityEvidence::capture(identity),
                heartbeat: heartbeat.clone(),
            },
            Some(identity),
        )
    }
    fn persist_broker_change(
        &self,
        change: Change,
        identity: Option<&BrokerIdentityV1>,
    ) -> anyhow::Result<BrokerMutationResultV1> {
        change.validate()?;
        let value = serde_json::to_value(&change)?;
        require(self.redactor.value(&value) == value)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        let received_at = format_time(now);
        if let Some(identity) = identity {
            permission(identity.tenant_id() == self.tenant_id)?;
            super::workload::revalidate_broker_identity(&tx, identity, now)?;
            let broker = load_broker(&tx, identity.broker_id())?
                .ok_or(BrokerRegistryFailureV1::PermissionDenied)?;
            broker.require_current_grant(identity, &received_at)?;
            if let Change::Heartbeat { heartbeat, .. } = &change {
                permission(heartbeat.incarnation_id == broker.incarnation_id)?;
            }
        }
        // Revocation/expiry/binding validation precede even an exact replay.
        if let Some(result) = replay(&tx, &change)? {
            self.commit_authorized(tx, &clock)?;
            return Ok(result);
        }
        let record = Record {
            version: 1,
            input_hash: change.input_hash()?,
            received_at,
            change,
        };
        super::workload::validate_control_time(&tx, &record.received_at, None)?;
        let mut projection = preflight(&tx, &record.change)?;
        let next_seq = audit_head(&tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let broker = projection.apply(&record, &self.tenant_id, next_seq)?;
        let mut event = Event::new(record.change.kind());
        event.timestamp = record.received_at.clone();
        event.data = serde_json::to_value(&record)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        require(serde_json::to_value(&event)? == original)?;
        let stored = if let Some(identity) = identity {
            self.append_authorized_workload_in_transaction(
                &tx,
                event,
                payload,
                super::capacity::WorkloadAuditPrincipalV1::Broker {
                    broker_id: identity.broker_id().to_owned(),
                },
                &clock,
            )?
        } else if matches!(record.change, Change::Revoke { .. }) {
            self.append_authorized_identity_in_transaction(&tx, event, payload, &clock)?
        } else {
            self.append_authorized_in_transaction(&tx, event, payload, &clock)?
        };
        require(
            stored.seq == next_seq
                && parse_time(&record.received_at)? <= parse_time(&stored.received_at)?,
        )?;
        tx.execute("INSERT INTO broker_registry_brokers(broker_id,grant_revision,heartbeat_revision,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(broker_id) DO UPDATE SET grant_revision=excluded.grant_revision,heartbeat_revision=excluded.heartbeat_revision,payload_json=excluded.payload_json,event_seq=excluded.event_seq", params![broker.broker_id,i64::try_from(broker.grant_revision)?,i64::try_from(broker.heartbeat_revision)?,serde_json::to_string(&broker)?,stored.seq])?;
        if let Change::Grant { mutation } = &record.change {
            for authority in &mutation.authorities {
                tx.execute("INSERT INTO broker_registry_authorities(authority_id,broker_id,runtime_kind) VALUES(?1,?2,?3) ON CONFLICT(authority_id) DO NOTHING", params![authority.authority_id,broker.broker_id,runtime_name(authority.runtime_kind)?])?;
            }
            tx.execute("INSERT INTO broker_registry_incarnations(incarnation_id,broker_id,event_seq) VALUES(?1,?2,?3)", params![mutation.incarnation_id,broker.broker_id,stored.seq])?;
        }
        tx.execute("INSERT INTO broker_registry_operations(operation_id,input_hash,event_seq,broker_id,result_json) VALUES(?1,?2,?3,?4,?5)", params![record.change.operation_id(),record.input_hash,stored.seq,broker.broker_id,serde_json::to_string(&broker)?])?;
        self.commit_authorized(tx, &clock)?;
        Ok(result(
            &stored,
            broker,
            record.input_hash,
            record.change.operation_id().into(),
            false,
        ))
    }
    pub fn broker(&self, id: &str) -> anyhow::Result<Option<BrokerViewV1>> {
        registry_uuid(id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let now = format_time(Utc::now());
        let broker = load_broker(&tx, id)?
            .map(|broker| broker.view_at(&now))
            .transpose()?;
        self.commit_checked(tx)?;
        Ok(broker)
    }
    /// Authenticated self metadata. The proof and snapshot share one checked
    /// transaction; reading it cannot refresh receipt liveness or runtime state.
    pub fn broker_for_identity(&self, identity: &BrokerIdentityV1) -> anyhow::Result<BrokerViewV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        super::workload::revalidate_broker_identity(&tx, identity, now)?;
        let broker = load_broker(&tx, identity.broker_id())?
            .ok_or(BrokerRegistryFailureV1::PermissionDenied)?;
        broker.require_current_grant(identity, &format_time(now))?;
        let view = broker.view_at(&format_time(now))?;
        self.commit_authorized(tx, &clock)?;
        Ok(view)
    }

    /// Resolve only this current broker proof's scope in one checked snapshot.
    /// This does not refresh a heartbeat or authorize a later write.
    pub fn broker_authenticated_identity(
        &self,
        identity: &BrokerIdentityV1,
    ) -> anyhow::Result<BrokerIdentityViewV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        permission(identity.tenant_id() == self.tenant_id)?;
        super::workload::revalidate_broker_identity(&tx, identity, now)?;
        let broker = load_broker(&tx, identity.broker_id())?
            .ok_or(BrokerRegistryFailureV1::PermissionDenied)?;
        broker.require_current_grant(identity, &format_time(now))?;
        let (tenant_id, binding_version): (String, i64) = tx.query_row(
            "SELECT tenant_id,schema_version FROM tenant_binding WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        permission(tenant_id == identity.tenant_id() && binding_version == 1)?;
        let namespace: String = tx.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        registry_uuid(&namespace)?;
        let schema: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let view = BrokerIdentityViewV1 {
            version: 1,
            tenant_id,
            control_namespace_uuid: namespace,
            schema_version: schema,
            broker_id: broker.broker_id,
            incarnation_id: broker.incarnation_id,
            credential_id: identity.credential_id().to_owned(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        };
        self.commit_authorized(tx, &clock)?;
        Ok(view)
    }

    /// Immutable ownership is historical provenance, never execution permission.
    /// An authority removed from the current grant retains its original owner.
    pub fn broker_for_authority(&self, authority_id: &str) -> anyhow::Result<Option<BrokerViewV1>> {
        require(
            !authority_id.is_empty()
                && authority_id.len() <= 256
                && !authority_id.chars().any(char::is_control),
        )?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let id = tx
            .query_row(
                "SELECT broker_id FROM broker_registry_authorities WHERE authority_id=?1",
                [authority_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let now = format_time(Utc::now());
        let broker = if let Some(id) = id {
            load_broker(&tx, &id)?
                .map(|broker| broker.view_at(&now))
                .transpose()?
        } else {
            None
        };
        self.commit_checked(tx)?;
        Ok(broker)
    }
    pub fn brokers_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<BrokerViewV1>> {
        require((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            registry_uuid(cursor)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let now = format_time(Utc::now());
        let brokers = {
            let mut statement = tx.prepare("SELECT payload_json FROM broker_registry_brokers WHERE broker_id>?1 ORDER BY broker_id LIMIT ?2")?;
            let rows = statement
                .query_map(params![cursor.unwrap_or(""), i64::from(limit)], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|json| serde_json::from_str::<BrokerV1>(&json)?.view_at(&now))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(brokers)
    }
}

#[cfg(test)]
#[path = "../broker_registry/replay_tests.rs"]
mod tests;

#[cfg(test)]
mod identity_view_tests {
    use super::*;
    use crate::workload::{
        AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadBindingRevocationV1,
        WorkloadFailureV1, WorkloadSubjectV1,
    };
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
    use uuid::Uuid;

    const BROKER: &str = "22222222-2222-4222-8222-222222222222";
    const CREDENTIAL: &str = "44444444-4444-4444-8444-444444444444";
    const INCARNATION: &str = "33333333-3333-4333-8333-333333333333";
    const ORIGINAL: &str = "public-identity-view-original-fixture-key-0000000000";
    const REPLACEMENT: &str = "public-identity-view-replacement-fixture-key-0000000";

    fn issue(store: &Store, credential: &str, incarnation: &str, revision: u64, token: &str) {
        store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: credential.into(),
                expected_revision: revision,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: BROKER.into(),
                    incarnation_id: incarnation.into(),
                },
                sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                expires_at: "2100-01-01T00:00:00.000Z".into(),
            })
            .unwrap();
    }

    fn grant(store: &Store, credential: &str, incarnation: &str, revision: Option<u64>) {
        store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: BROKER.into(),
                expected_revision: revision,
                credential_id: credential.into(),
                incarnation_id: incarnation.into(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: "identity-view-declared-authority".into(),
                    runtime_kind: BrokerRuntimeKindV1::Docker,
                }],
                lease_seconds: 1,
                not_after: "2100-01-01T00:00:00.000Z".into(),
            })
            .unwrap();
    }

    fn fixture() -> (tempfile::TempDir, Store) {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("tenant.sqlite");
        let store =
            Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
        store
            .migrate_control_v3(&path.with_extension("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&path.with_extension("before6"))
            .unwrap();
        issue(&store, CREDENTIAL, INCARNATION, 0, ORIGINAL);
        grant(&store, CREDENTIAL, INCARNATION, None);
        (directory, store)
    }

    fn extract(store: &Store, token: &str) -> BrokerIdentityV1 {
        match store.authenticate_workload_token(token).unwrap().unwrap() {
            AuthenticatedWorkload::Broker(identity) => identity,
            _ => panic!("fixture must authenticate a real broker proof"),
        }
    }

    #[test]
    fn identity_view_revalidates_extracted_proof_after_binding_rotation_and_revocation() {
        let (_directory, store) = fixture();
        let original = extract(&store, ORIGINAL);
        assert_eq!(
            store
                .broker_authenticated_identity(&original)
                .unwrap()
                .binding_revision,
            1
        );
        issue(&store, CREDENTIAL, INCARNATION, 1, REPLACEMENT);
        assert!(
            store.broker_authenticated_identity(&original).is_err(),
            "a previously extracted binding revision is no longer authority"
        );
        let current = extract(&store, REPLACEMENT);
        assert_eq!(
            store
                .broker_authenticated_identity(&current)
                .unwrap()
                .binding_revision,
            2
        );
        store
            .revoke_workload_binding(&WorkloadBindingRevocationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: CREDENTIAL.into(),
                expected_revision: 2,
            })
            .unwrap();
        let head = store.head().unwrap();
        assert!(
            store.broker_authenticated_identity(&current).is_err(),
            "revocation must invalidate an already extracted proof"
        );
        assert_eq!(store.head().unwrap(), head);
    }

    #[tokio::test]
    async fn identity_view_ignores_heartbeat_staleness_but_revalidates_replaced_or_revoked_grants()
    {
        let (_directory, store) = fixture();
        let original = extract(&store, ORIGINAL);
        assert_eq!(
            store.broker(BROKER).unwrap().unwrap().state,
            BrokerStateV1::Unseen
        );
        assert!(store.broker_authenticated_identity(&original).is_ok());
        let heartbeat = store
            .broker_heartbeat(
                &original,
                &BrokerHeartbeatV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    expected_heartbeat_revision: 0,
                    incarnation_id: INCARNATION.into(),
                    sequence: 1,
                },
            )
            .unwrap();
        let expiry =
            parse_time(&heartbeat.broker.last_heartbeat.unwrap().lease_expires_at).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while Utc::now() < expiry {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture heartbeat lease must expire as a functional condition");
        assert_eq!(
            store.broker(BROKER).unwrap().unwrap().state,
            BrokerStateV1::Stale
        );
        let before = store.head().unwrap();
        assert!(
            store.broker_authenticated_identity(&original).is_ok(),
            "a stale heartbeat is not a revoked broker grant"
        );
        assert_eq!(
            store.head().unwrap(),
            before,
            "identity must not refresh heartbeat liveness"
        );
        let credential = Uuid::new_v4().to_string();
        let incarnation = Uuid::new_v4().to_string();
        issue(&store, &credential, &incarnation, 0, REPLACEMENT);
        grant(&store, &credential, &incarnation, Some(1));
        assert!(
            store.broker_authenticated_identity(&original).is_err(),
            "an extracted proof cannot follow a replaced grant or incarnation"
        );
        let current = extract(&store, REPLACEMENT);
        let view = store.broker_authenticated_identity(&current).unwrap();
        assert_eq!(view.grant_revision, 2);
        assert_eq!(view.incarnation_id, incarnation);
        store
            .revoke_broker(&BrokerRevocationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: BROKER.into(),
                expected_revision: 2,
            })
            .unwrap();
        let head = store.head().unwrap();
        assert!(
            store.broker_authenticated_identity(&current).is_err(),
            "grant revocation also invalidates an already extracted proof"
        );
        assert_eq!(store.head().unwrap(), head);
    }

    #[test]
    fn identity_view_refuses_an_extracted_proof_after_a_future_durable_time_floor() {
        let (_directory, store) = fixture();
        let identity = extract(&store, ORIGINAL);
        assert!(store.broker_authenticated_identity(&identity).is_ok());
        let at = parse_time(&format_time(Utc::now() + chrono::Duration::days(1))).unwrap();
        let mut db = store.db.lock().unwrap();
        let tx = store.begin_checked(&mut db).unwrap();
        let mut event = Event::new("test.broker.identity.clock.received");
        event.timestamp = format_time(at);
        let (event, payload) = store.prepare_event(event).unwrap();
        store
            .append_with_capacity_class_at(
                &tx,
                event,
                payload,
                super::super::capacity::Class::OrdinaryWrite,
                at,
            )
            .unwrap();
        store.commit_checked(tx).unwrap();
        drop(db);
        let head = store.head().unwrap();
        let failure = store.broker_authenticated_identity(&identity).unwrap_err();
        assert_eq!(
            failure.downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert_eq!(
            store.head().unwrap(),
            head,
            "a clock refusal must not publish new evidence"
        );
        assert!(store.integrity().unwrap().valid);
    }
}
