//! Relay projections and mandatory audit share the tenant's checked SQLite transaction.
use super::*;
use crate::control::uuid;
use crate::relay::*;
use chrono::{DateTime, Duration, Utc};

mod retention;

#[cfg(test)]
mod history_compat_tests;

pub(super) const SCHEMA: [(&str, &str, &str); 6] = [
    (
        "table",
        "relay_grants",
        "CREATE TABLE relay_grants(grant_id TEXT PRIMARY KEY,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "table",
        "relay_messages",
        "CREATE TABLE relay_messages(message_id TEXT PRIMARY KEY,grant_id TEXT NOT NULL REFERENCES relay_grants(grant_id),metadata_json TEXT NOT NULL,body TEXT,accepted_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),event_seq INTEGER NOT NULL REFERENCES events(seq),destination_worker_id TEXT NOT NULL,destination_attempt_id TEXT NOT NULL,destination_generation INTEGER NOT NULL CHECK(destination_generation=1),body_bytes INTEGER NOT NULL CHECK(body_bytes>0 AND body_bytes<=65536),acknowledged INTEGER NOT NULL CHECK(acknowledged IN (0,1)),expires_at TEXT NOT NULL,received_at TEXT NOT NULL)",
    ),
    (
        "table",
        "relay_operations",
        "CREATE TABLE relay_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,action TEXT NOT NULL,resource_id TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "relay_messages_retained",
        "CREATE INDEX relay_messages_retained ON relay_messages(body_bytes) WHERE body IS NOT NULL",
    ),
    (
        "index",
        "relay_messages_rate",
        "CREATE INDEX relay_messages_rate ON relay_messages(grant_id,received_at)",
    ),
    (
        "index",
        "relay_messages_inbox",
        "CREATE INDEX relay_messages_inbox ON relay_messages(destination_worker_id,destination_attempt_id,destination_generation,accepted_event_seq,expires_at) WHERE body IS NOT NULL AND acknowledged=0",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 3] = [
    ("relay_grants", 1),
    ("relay_messages", 2),
    ("relay_operations", 2),
];

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Grant {
    grant_id: String,
    source: RelayEndpointV1,
    destination: RelayEndpointV1,
    channel: String,
    expires_at: String,
    revoked: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "metadata",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Resource {
    Grant(Grant),
    Message(RelayMessageMetadataV1),
}
impl Resource {
    fn id(&self) -> &str {
        match self {
            Self::Grant(v) => &v.grant_id,
            Self::Message(v) => &v.message_id,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    operation_id: String,
    input_hash: String,
    action: String,
    actor: Option<Actor>,
    resource: Resource,
}

/// Serializable historical attribution cannot itself authorize a live request.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Actor {
    credential_id: String,
    endpoint: RelayEndpointV1,
    binding_revision: u64,
}
impl Actor {
    fn from_verified(identity: &WorkerRelayIdentity) -> Self {
        Self {
            credential_id: identity.credential_id.clone(),
            endpoint: RelayEndpointV1 {
                worker_id: identity.worker_id.clone(),
                attempt_id: identity.attempt_id.clone(),
                generation: identity.generation,
            },
            binding_revision: identity.binding_revision,
        }
    }
    fn validate_at(
        &self,
        db: &Connection,
        tenant: &str,
        at: DateTime<Utc>,
        before_seq: i64,
    ) -> anyhow::Result<()> {
        let identity = WorkerRelayIdentity {
            tenant_id: tenant.into(),
            credential_id: self.credential_id.clone(),
            worker_id: self.endpoint.worker_id.clone(),
            attempt_id: self.endpoint.attempt_id.clone(),
            generation: self.endpoint.generation,
            binding_revision: self.binding_revision,
        };
        super::workload::validate_worker_relay_identity_at(db, &identity, at, before_seq)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRecord {
    version: u32,
    actor: Actor,
    after: Option<i64>,
    limit: u32,
    message_ids: Vec<String>,
}

/// Recover the private workload budget identity from closed, typed relay audit records.
/// This is not an authority constructor: request fields never select an audit class.
pub(super) fn workload_audit_identity(
    event: &Event,
) -> anyhow::Result<Option<super::capacity::WorkloadAuditPrincipalV1>> {
    let actor = match event.kind.as_str() {
        "control.relay.read" => {
            let read: ReadRecord = serde_json::from_value(event.data.clone())?;
            valid(
                read.version == 1
                    && read.limit > 0
                    && read.limit <= 32
                    && read.message_ids.len() <= read.limit as usize,
            )?;
            read.actor
        }
        "control.relay.mutation" => {
            let record: Record = serde_json::from_value(event.data.clone())?;
            valid(record.version == 1)?;
            match (record.action.as_str(), record.resource) {
                ("send", Resource::Message(message)) => {
                    let actor = record.actor.context("relay send audit actor missing")?;
                    valid(actor.endpoint == message.source)?;
                    actor
                }
                ("ack", Resource::Message(message)) => {
                    let actor = record
                        .actor
                        .context("relay acknowledgement audit actor missing")?;
                    valid(actor.endpoint == message.destination)?;
                    actor
                }
                ("grant" | "revoke_grant", Resource::Grant(_))
                | ("prune", Resource::Message(_)) => {
                    valid(record.actor.is_none())?;
                    return Ok(None);
                }
                _ => anyhow::bail!("invalid relay audit budget attribution"),
            }
        }
        _ if event.kind.starts_with("control.relay.") => {
            anyhow::bail!("unknown authoritative relay event")
        }
        _ => return Ok(None),
    };
    uuid(&actor.credential_id)?;
    actor.endpoint.validate()?;
    valid(actor.binding_revision > 0)?;
    Ok(Some(super::capacity::WorkloadAuditPrincipalV1::Worker {
        worker_id: actor.endpoint.worker_id,
    }))
}

pub(super) fn initialize(db: &Connection) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        db.execute_batch(sql)?;
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn input_hash(action: &str, input: &impl Serialize) -> anyhow::Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        "ai-guard:relay:v1",
        action,
        input,
    ))?))
}
fn timestamp(value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}
fn canonical_time(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn require_relay(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    anyhow::ensure!(
        version >= 6,
        "worker relay requires explicit schema migration"
    );
    Ok(())
}
fn forbidden(condition: bool) -> anyhow::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(RelayFailure::Forbidden.into())
    }
}
fn conflict(condition: bool) -> anyhow::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(RelayFailure::Conflict.into())
    }
}
fn request(version: u32, operation: &str) -> anyhow::Result<()> {
    valid(version == 1)?;
    uuid(operation)
}

fn grant_record_at(
    input: &RelayGrantV1,
    expires: DateTime<Utc>,
    now: DateTime<Utc>,
    hash: String,
) -> anyhow::Result<Record> {
    valid(expires > now && expires <= now + Duration::seconds(MAX_GRANT_SECONDS))?;
    let expires_at = canonical_time(expires);
    let durable_now = timestamp(&canonical_time(now))?;
    let durable_expires = timestamp(&expires_at)?;
    valid(
        durable_expires > durable_now
            && durable_expires <= durable_now + Duration::seconds(MAX_GRANT_SECONDS),
    )?;
    Ok(Record {
        version: 1,
        operation_id: input.operation_id.clone(),
        input_hash: hash,
        action: "grant".into(),
        actor: None,
        resource: Resource::Grant(Grant {
            grant_id: input.grant_id.clone(),
            source: input.source.clone(),
            destination: input.destination.clone(),
            channel: input.channel.clone(),
            expires_at,
            revoked: false,
        }),
    })
}
fn load_json<T: for<'de> Deserialize<'de>>(
    db: &Connection,
    query: &str,
    id: &str,
) -> anyhow::Result<Option<T>> {
    let payload: Option<String> = db.query_row(query, [id], |r| r.get(0)).optional()?;
    payload
        .map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
fn grant(db: &Connection, id: &str) -> anyhow::Result<Option<Grant>> {
    load_json(
        db,
        "SELECT payload_json FROM relay_grants WHERE grant_id=?1",
        id,
    )
}
fn message(db: &Connection, id: &str) -> anyhow::Result<Option<RelayMessageMetadataV1>> {
    load_json(
        db,
        "SELECT metadata_json FROM relay_messages WHERE message_id=?1",
        id,
    )
}
fn active_grant_at(db: &Connection, grant: &Grant, now: DateTime<Utc>) -> anyhow::Result<()> {
    super::workload::validate_control_time_floor(db, now, None)?;
    forbidden(!grant.revoked && timestamp(&grant.expires_at)? > now)
}

#[derive(Debug, PartialEq, Eq)]
struct MessageIndex {
    worker: String,
    attempt: String,
    generation: i64,
    body_bytes: i64,
    acknowledged: bool,
    expires_at: String,
    received_at: String,
}
impl MessageIndex {
    fn expected(message: &RelayMessageMetadataV1, received_at: &str) -> anyhow::Result<Self> {
        Ok(Self {
            worker: message.destination.worker_id.clone(),
            attempt: message.destination.attempt_id.clone(),
            generation: i64::try_from(message.destination.generation)?,
            body_bytes: i64::try_from(message.body_bytes)?,
            acknowledged: message.acknowledged,
            expires_at: message.expires_at.clone(),
            received_at: received_at.into(),
        })
    }
    fn read(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            worker: row.get(offset)?,
            attempt: row.get(offset + 1)?,
            generation: row.get(offset + 2)?,
            body_bytes: row.get(offset + 3)?,
            acknowledged: row.get(offset + 4)?,
            expires_at: row.get(offset + 5)?,
            received_at: row.get(offset + 6)?,
        })
    }
}
struct MessageRow {
    id: String,
    grant: String,
    metadata: String,
    body: Option<String>,
    accepted: i64,
    last: i64,
    index: MessageIndex,
}
impl MessageRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            grant: row.get(1)?,
            metadata: row.get(2)?,
            body: row.get(3)?,
            accepted: row.get(4)?,
            last: row.get(5)?,
            index: MessageIndex::read(row, 6)?,
        })
    }
}
fn check_message_projection(
    row: &MessageRow,
    expected: &RelayMessageMetadataV1,
    accepted: i64,
    received: &str,
    last: i64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        row.id == expected.message_id
            && row.grant == expected.grant_id
            && row.metadata == serde_json::to_string(expected)?
            && row.accepted == accepted
            && row.last == last
            && row.index == MessageIndex::expected(expected, received)?,
        "relay message projection differs from audited metadata"
    );
    if expected.body_pruned {
        anyhow::ensure!(row.body.is_none(), "pruned relay message retains body");
    } else {
        let body = row.body.as_ref().context("retained relay body missing")?;
        anyhow::ensure!(
            body.len() as u64 == expected.body_bytes
                && digest(body.as_bytes()) == expected.body_sha256,
            "relay message body differs from audited digest"
        );
    }
    Ok(())
}
const MESSAGE_ROW_SQL: &str = "SELECT message_id,grant_id,metadata_json,body,accepted_event_seq,event_seq,destination_worker_id,destination_attempt_id,destination_generation,body_bytes,acknowledged,expires_at,received_at FROM relay_messages";
fn replay(
    db: &Connection,
    operation: &str,
    hash: &str,
    action: &str,
) -> anyhow::Result<Option<RelayReceiptV1>> {
    let row: Option<(String, String, String, i64, String)> = db.query_row(
        "SELECT o.input_hash,o.action,o.resource_id,e.seq,e.hash FROM relay_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
        [operation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
    row.map(|(old_hash, old_action, id, seq, audit_hash)| {
        conflict(old_hash == hash && old_action == action)?;
        Ok(RelayReceiptV1 {
            version: 1,
            operation_id: operation.into(),
            resource_id: id,
            audit_seq: seq,
            audit_hash,
            replayed: true,
        })
    })
    .transpose()
}

const RETAINED_USAGE_SQL: &str = "SELECT COUNT(*),COALESCE(SUM(body_bytes),0) FROM relay_messages INDEXED BY relay_messages_retained WHERE body IS NOT NULL";
const RECENT_GRANT_SQL: &str = "SELECT COUNT(*) FROM relay_messages INDEXED BY relay_messages_rate WHERE grant_id=?1 AND received_at>?2 AND received_at<=?3";
fn grant_rate_at(db: &Connection, grant_id: &str, now: DateTime<Utc>) -> anyhow::Result<()> {
    let recent: i64 = db.query_row(
        RECENT_GRANT_SQL,
        params![
            grant_id,
            canonical_time(now - Duration::seconds(60)),
            canonical_time(now)
        ],
        |r| r.get(0),
    )?;
    if u64::try_from(recent)? >= MAX_MESSAGES_PER_MINUTE {
        return Err(RelayFailure::RateLimited.into());
    }
    Ok(())
}
const INBOX_SQL: &str = "SELECT accepted_event_seq,metadata_json,body FROM relay_messages INDEXED BY relay_messages_inbox WHERE destination_worker_id=?1 AND destination_attempt_id=?2 AND destination_generation=?3 AND accepted_event_seq>?4 AND body IS NOT NULL AND acknowledged=0 AND expires_at>?5 ORDER BY accepted_event_seq LIMIT ?6";

/// Why a relay fact is written. Chosen by trusted methods, never by a request field.
#[derive(Clone, Copy)]
enum Origin<'a> {
    /// The authenticated caller's own operation.
    Request,
    /// Bounded release of an expired body inside an authorized send:
    /// `trigger` is that send's operation and `worker` its verified sender.
    AutomaticPrune { trigger: &'a str, worker: &'a str },
}

impl Store {
    /// Every relay fact is dated by its own `clock`, captured after the previous
    /// append of the transaction: the validator requires each fact to be at or
    /// after the reception of all earlier events.
    fn persist_relay(
        &self,
        tx: &Transaction<'_>,
        record: &Record,
        body: Option<&str>,
        clock: &super::workload::AuthorizedControlTimeV1,
        origin: Origin<'_>,
    ) -> anyhow::Result<RelayReceiptV1> {
        let mut event = Event::new("control.relay.mutation");
        event.timestamp = canonical_time(clock.at());
        if let Origin::AutomaticPrune { trigger, worker } = origin {
            // Provenance in a free-form field, not attribution: the record
            // keeps no actor, as the historical prune rule requires.
            event.correlation_id = Some(format!(
                "{AUTOMATIC_PRUNE_CORRELATION_PREFIX}{trigger}:{worker}"
            ));
        }
        event.data = serde_json::to_value(record)?;
        // Worker and agent identities are different. The typed audit metadata
        // carries the authenticated workload actor without inventing an agent ID.
        let raw = serde_json::to_value(&event)?;
        valid(self.redactor.value(&raw) == raw)?;
        let (event, payload) = self.prepare_event(event)?;
        let stored = if matches!(origin, Origin::AutomaticPrune { .. }) {
            anyhow::ensure!(
                record.action == "prune" && record.actor.is_none(),
                "automatic relay release must be an unattributed prune"
            );
            // Ordinary class: automatic cleanup can never draw on the control
            // or identity reserves that the runner's deliberate prune may use.
            self.append_with_authorized_capacity_class_at(
                tx,
                event,
                payload,
                super::capacity::Class::OrdinaryWrite,
                clock,
                Utc::now(),
            )?
        } else if matches!(record.action.as_str(), "send" | "ack") {
            let actor = record
                .actor
                .as_ref()
                .context("worker relay audit actor missing")?;
            self.append_authorized_workload_in_transaction(
                tx,
                event,
                payload,
                super::capacity::WorkloadAuditPrincipalV1::Worker {
                    worker_id: actor.endpoint.worker_id.clone(),
                },
                clock,
            )?
        } else if record.action == "revoke_grant" {
            // This action is chosen by the trusted runner method, never a worker DTO.
            self.append_authorized_identity_in_transaction(tx, event, payload, clock)?
        } else {
            self.append_authorized_in_transaction(tx, event, payload, clock)?
        };
        match &record.resource {
            Resource::Grant(value) => {
                tx.execute("INSERT INTO relay_grants(grant_id,payload_json,event_seq) VALUES(?1,?2,?3) ON CONFLICT(grant_id) DO UPDATE SET payload_json=excluded.payload_json,event_seq=excluded.event_seq",
                    params![value.grant_id, serde_json::to_string(value)?, stored.seq])?;
            }
            Resource::Message(value) if record.action == "send" => {
                tx.execute("INSERT INTO relay_messages(message_id,grant_id,metadata_json,body,accepted_event_seq,event_seq,destination_worker_id,destination_attempt_id,destination_generation,body_bytes,acknowledged,expires_at,received_at) VALUES(?1,?2,?3,?4,?5,?5,?6,?7,?8,?9,?10,?11,?12)",
                    params![value.message_id, value.grant_id, serde_json::to_string(value)?, body.context("relay body missing")?, stored.seq,value.destination.worker_id,value.destination.attempt_id,i64::try_from(value.destination.generation)?,i64::try_from(value.body_bytes)?,value.acknowledged,value.expires_at,stored.received_at])?;
            }
            Resource::Message(value) => {
                let changed = tx.execute("UPDATE relay_messages SET metadata_json=?2,body=CASE WHEN ?3 THEN NULL ELSE body END,event_seq=?4,acknowledged=?5 WHERE message_id=?1",
                    params![value.message_id, serde_json::to_string(value)?, value.body_pruned, stored.seq,value.acknowledged])?;
                anyhow::ensure!(changed == 1, "relay message update lost");
            }
        }
        tx.execute("INSERT INTO relay_operations(operation_id,input_hash,action,resource_id,event_seq) VALUES(?1,?2,?3,?4,?5)",
            params![record.operation_id, record.input_hash, record.action, record.resource.id(), stored.seq])?;
        // Exact local write checks keep normal work bounded. Full reconstruction is
        // performed by the control validator at startup or after an external change.
        match &record.resource {
            Resource::Grant(expected) => {
                let (actual, event_seq): (String, i64) = tx.query_row(
                    "SELECT payload_json,event_seq FROM relay_grants WHERE grant_id=?1",
                    [&expected.grant_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                anyhow::ensure!(
                    actual == serde_json::to_string(expected)? && event_seq == stored.seq,
                    "relay grant write diverged"
                );
            }
            Resource::Message(expected) => {
                let row = tx.query_row(
                    &format!("{MESSAGE_ROW_SQL} WHERE message_id=?1"),
                    [&expected.message_id],
                    MessageRow::read,
                )?;
                let (accepted_expected,received_expected):(i64,String) = tx.query_row("SELECT o.event_seq,e.received_at FROM relay_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1 AND o.action='send'", [&expected.message_id], |r|Ok((r.get(0)?,r.get(1)?)))?;
                check_message_projection(
                    &row,
                    expected,
                    accepted_expected,
                    &received_expected,
                    stored.seq,
                )?;
            }
        }
        let actual = replay(tx, &record.operation_id, &record.input_hash, &record.action)?
            .context("relay operation write missing")?;
        anyhow::ensure!(
            actual.resource_id == record.resource.id()
                && actual.audit_seq == stored.seq
                && actual.audit_hash == stored.hash,
            "relay operation write diverged"
        );
        Ok(RelayReceiptV1 {
            version: 1,
            operation_id: record.operation_id.clone(),
            resource_id: record.resource.id().into(),
            audit_seq: stored.seq,
            audit_hash: stored.hash,
            replayed: false,
        })
    }

    pub fn relay_grant(&self, input: &RelayGrantV1) -> anyhow::Result<RelayReceiptV1> {
        self.relay_grant_with_access(None, input)
    }
    pub(crate) fn relay_grant_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        input: &RelayGrantV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_grant_with_access(Some(access), input)
    }
    fn relay_grant_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        input: &RelayGrantV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        request(input.version, &input.operation_id)?;
        uuid(&input.grant_id)?;
        input.source.validate()?;
        input.destination.validate()?;
        valid(
            channel_valid(&input.channel) && input.source.worker_id != input.destination.worker_id,
        )?;
        let expires = timestamp(&input.expires_at)?;
        let hash = input_hash("grant", input)?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        for endpoint in [&input.source, &input.destination] {
            self.checked_worker_relay_endpoint(&tx, endpoint, now)?;
        }
        if let Some(receipt) = replay(&tx, &input.operation_id, &hash, "grant")? {
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        conflict(grant(&tx, &input.grant_id)?.is_none())?;
        let record = grant_record_at(input, expires, now, hash)?;
        let result = self.persist_relay(&tx, &record, None, &clock, Origin::Request)?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }

    pub fn relay_revoke(&self, input: &RelayRevokeV1) -> anyhow::Result<RelayReceiptV1> {
        self.relay_revoke_with_access(None, input)
    }
    pub(crate) fn relay_revoke_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        input: &RelayRevokeV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_revoke_with_access(Some(access), input)
    }
    fn relay_revoke_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        input: &RelayRevokeV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        request(input.version, &input.operation_id)?;
        uuid(&input.grant_id)?;
        let hash = input_hash("revoke_grant", input)?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        if let Some(receipt) = replay(&tx, &input.operation_id, &hash, "revoke_grant")? {
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        let mut value = grant(&tx, &input.grant_id)?.ok_or(RelayFailure::NotFound)?;
        value.revoked = true;
        let record = Record {
            version: 1,
            operation_id: input.operation_id.clone(),
            input_hash: hash,
            action: "revoke_grant".into(),
            actor: None,
            resource: Resource::Grant(value),
        };
        let result = self.persist_relay(&tx, &record, None, &clock, Origin::Request)?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }

    pub fn relay_send(
        &self,
        identity: &WorkerRelayIdentity,
        input: &RelaySendV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_send_with_access(None, identity, input)
    }
    pub(crate) fn relay_send_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        identity: &WorkerRelayIdentity,
        input: &RelaySendV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_send_with_access(Some(access), identity, input)
    }
    fn relay_send_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        identity: &WorkerRelayIdentity,
        input: &RelaySendV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        request(input.version, &input.operation_id)?;
        uuid(&input.grant_id)?;
        valid(
            !input.body.is_empty()
                && input.body.len() <= MAX_MESSAGE_BYTES
                && input.ttl_seconds > 0
                && input.ttl_seconds <= MAX_MESSAGE_TTL_SECONDS,
        )?;
        valid(self.redactor.text(&input.body) == input.body)?;
        if let Ok(structured) = serde_json::from_str::<Value>(&input.body) {
            valid(self.redactor.value_unclipped(&structured) == structured)?;
        }
        let body_sha256 = digest(input.body.as_bytes());
        let hash = input_hash(
            "send",
            &(
                input.version,
                &input.operation_id,
                &input.grant_id,
                input.ttl_seconds,
                &body_sha256,
                &identity.credential_id,
            ),
        )?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let (source, grant) =
            self.relay_send_authority(&tx, identity, &input.grant_id, clock.at())?;
        if let Some(receipt) = replay(&tx, &input.operation_id, &hash, "send")? {
            let retained = message(&tx, &receipt.resource_id)?
                .context("relay send receipt has no message tombstone")?;
            forbidden(
                retained.source == source
                    && retained.destination == grant.destination
                    && retained.grant_id == grant.grant_id,
            )?;
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        grant_rate_at(&tx, &grant.grant_id, clock.at())?;
        let mut retained = retention::Retained::load(&tx)?;
        let cleanup =
            self.relay_release_expired(&tx, &mut retained, &source, &input.operation_id, clock)?;
        let admitted = (|| -> anyhow::Result<RelayReceiptV1> {
            let now = cleanup.clock.at();
            let (source, grant) = if cleanup.released > 0 {
                // The send fact follows the release facts: its authority, rate
                // window, creation and expiry are all evaluated at its own instant.
                let authority = self.relay_send_authority(&tx, identity, &input.grant_id, now)?;
                grant_rate_at(&tx, &authority.1.grant_id, now)?;
                authority
            } else {
                (source, grant)
            };
            retention::admit(
                &self.relay_retention_policy,
                &retained,
                &source,
                &grant.grant_id,
                input.body.len() as u64,
                now,
            )?;
            let expires = std::cmp::min(
                timestamp(&grant.expires_at)?,
                now + Duration::seconds(i64::from(input.ttl_seconds)),
            );
            if canonical_time(expires) <= canonical_time(now) {
                return Err(RelayFailure::Expired.into());
            }
            let metadata = RelayMessageMetadataV1 {
                version: 1,
                message_id: input.operation_id.clone(),
                grant_id: grant.grant_id,
                source: grant.source,
                destination: grant.destination,
                channel: grant.channel,
                created_at: canonical_time(now),
                expires_at: canonical_time(expires),
                body_bytes: input.body.len() as u64,
                body_sha256,
                acknowledged: false,
                body_pruned: false,
            };
            let record = Record {
                version: 1,
                operation_id: input.operation_id.clone(),
                input_hash: hash,
                action: "send".into(),
                actor: Some(Actor::from_verified(identity)),
                resource: Resource::Message(metadata),
            };
            self.persist_relay(
                &tx,
                &record,
                Some(&input.body),
                &cleanup.clock,
                Origin::Request,
            )
        })();
        match admitted {
            Ok(receipt) => {
                self.commit_authorized(tx, &cleanup.clock)?;
                Ok(receipt)
            }
            // Each release is complete and audited by its own fact. Keeping it
            // when the send itself is refused lets a retry progress instead of
            // repeating the same releases, and avoids a rolled-back write.
            Err(refusal) if cleanup.survives(&tx, &refusal) => {
                self.commit_authorized(tx, &cleanup.clock)?;
                Err(refusal)
            }
            Err(error) => Err(error),
        }
    }

    /// Current authority of a send at `now`: verified source identity, its
    /// active directed grant and a currently authorized destination endpoint.
    fn relay_send_authority(
        &self,
        tx: &CheckedTransaction<'_>,
        identity: &WorkerRelayIdentity,
        grant_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(RelayEndpointV1, Grant)> {
        let source = self.checked_worker_relay_identity(tx, identity, now)?;
        let grant = grant(tx, grant_id)?.ok_or(RelayFailure::Forbidden)?;
        forbidden(grant.source == source)?;
        active_grant_at(tx, &grant, now)?;
        self.checked_worker_relay_endpoint(tx, &grant.destination, now)?;
        Ok((source, grant))
    }

    pub fn relay_inbox(
        &self,
        identity: &WorkerRelayIdentity,
        after: Option<i64>,
        limit: u32,
    ) -> anyhow::Result<RelayInboxV1> {
        self.relay_inbox_with_access(None, identity, after, limit)
    }
    pub(crate) fn relay_inbox_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        identity: &WorkerRelayIdentity,
        after: Option<i64>,
        limit: u32,
    ) -> anyhow::Result<RelayInboxV1> {
        self.relay_inbox_with_access(Some(access), identity, after, limit)
    }
    fn relay_inbox_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        identity: &WorkerRelayIdentity,
        after: Option<i64>,
        limit: u32,
    ) -> anyhow::Result<RelayInboxV1> {
        valid(after.is_none_or(|v| v >= 0) && limit > 0 && limit <= 32)?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        let destination = self.checked_worker_relay_identity(&tx, identity, now)?;
        let mut stmt = tx.prepare(INBOX_SQL)?;
        let mut rows = stmt.query(params![
            destination.worker_id,
            destination.attempt_id,
            i64::try_from(destination.generation)?,
            after.unwrap_or(0),
            canonical_time(now),
            i64::try_from(MAX_RETAINED_MESSAGES)?
        ])?;
        let mut result = Vec::new();
        // JSON array brackets plus room for the small HTTP inbox envelope.
        let mut encoded_bytes = 66_usize;
        let mut more = false;
        while let Some(row) = rows.next()? {
            let metadata: RelayMessageMetadataV1 = serde_json::from_str(&row.get::<_, String>(1)?)?;
            if metadata.destination != destination
                || metadata.acknowledged
                || timestamp(&metadata.expires_at)? <= now
            {
                continue;
            }
            let grant = grant(&tx, &metadata.grant_id)?.ok_or(RelayFailure::Forbidden)?;
            match active_grant_at(&tx, &grant, now) {
                Ok(()) => (),
                Err(error)
                    if error.downcast_ref::<RelayFailure>() == Some(&RelayFailure::Forbidden) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
            // Revocation/termination at either endpoint stops delivery of queued messages too.
            match self.checked_worker_relay_endpoint(&tx, &grant.source, now) {
                Ok(()) => (),
                Err(error)
                    if error.downcast_ref::<RelayFailure>() == Some(&RelayFailure::Forbidden) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
            let next = RelayMessageV1 {
                sequence: row.get(0)?,
                metadata,
                body: row.get(2)?,
            };
            let next_bytes = serde_json::to_vec(&next)?.len() + usize::from(!result.is_empty());
            if result.len() >= limit as usize || encoded_bytes + next_bytes > MAX_INBOX_BYTES {
                more = true;
                break;
            }
            encoded_bytes += next_bytes;
            result.push(next);
        }
        drop(rows);
        drop(stmt);
        // A content read is audited before its body leaves trusted control.
        let mut event = Event::new("control.relay.read");
        event.timestamp = canonical_time(now);
        event.data = serde_json::to_value(ReadRecord {
            version: 1,
            actor: Actor::from_verified(identity),
            after,
            limit,
            message_ids: result
                .iter()
                .map(|v| v.metadata.message_id.clone())
                .collect(),
        })?;
        let raw = serde_json::to_value(&event)?;
        valid(self.redactor.value(&raw) == raw)?;
        let (event, payload) = self.prepare_event(event)?;
        self.append_authorized_workload_in_transaction(
            &tx,
            event,
            payload,
            super::capacity::WorkloadAuditPrincipalV1::Worker {
                worker_id: identity.worker_id.clone(),
            },
            &clock,
        )?;
        self.commit_authorized(tx, &clock)?;
        let next_after = if more {
            result.last().map(|message| message.sequence)
        } else {
            None
        };
        Ok(RelayInboxV1 {
            version: 1,
            items: result,
            next_after,
        })
    }

    pub fn relay_ack(
        &self,
        identity: &WorkerRelayIdentity,
        input: &RelayAckV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_ack_with_access(None, identity, input)
    }
    pub(crate) fn relay_ack_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        identity: &WorkerRelayIdentity,
        input: &RelayAckV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_ack_with_access(Some(access), identity, input)
    }
    fn relay_ack_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        identity: &WorkerRelayIdentity,
        input: &RelayAckV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        request(input.version, &input.operation_id)?;
        uuid(&input.message_id)?;
        let hash = input_hash("ack", &(input, &identity.credential_id))?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        let now = clock.at();
        let destination = self.checked_worker_relay_identity(&tx, identity, now)?;
        let mut value = message(&tx, &input.message_id)?.ok_or(RelayFailure::NotFound)?;
        forbidden(value.destination == destination)?;
        let grant = grant(&tx, &value.grant_id)?.ok_or(RelayFailure::Forbidden)?;
        forbidden(grant.destination == destination)?;
        active_grant_at(&tx, &grant, now)?;
        self.checked_worker_relay_endpoint(&tx, &grant.source, now)?;
        if let Some(receipt) = replay(&tx, &input.operation_id, &hash, "ack")? {
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        if value.body_pruned || timestamp(&value.expires_at)? <= now {
            return Err(RelayFailure::Expired.into());
        }
        value.acknowledged = true;
        if self.relay_retention_policy.release_on_ack {
            // Explicit operator opt-in: this one fact, attributed to the
            // recipient, also retires the body. No separate prune is written.
            value.body_pruned = true;
        }
        let record = Record {
            version: 1,
            operation_id: input.operation_id.clone(),
            input_hash: hash,
            action: "ack".into(),
            actor: Some(Actor::from_verified(identity)),
            resource: Resource::Message(value),
        };
        let result = self.persist_relay(&tx, &record, None, &clock, Origin::Request)?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }

    /// Remove one expired body atomically with metadata-only audit. Tombstones preserve idempotence.
    /// A caller retries the same operation/message pair; a different pair is a conflict.
    pub fn relay_prune(&self, input: &RelayPruneV1) -> anyhow::Result<RelayReceiptV1> {
        self.relay_prune_with_access(None, input)
    }
    pub(crate) fn relay_prune_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        input: &RelayPruneV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        self.relay_prune_with_access(Some(access), input)
    }
    fn relay_prune_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        input: &RelayPruneV1,
    ) -> anyhow::Result<RelayReceiptV1> {
        request(input.version, &input.operation_id)?;
        uuid(&input.message_id)?;
        let hash = input_hash("prune", input)?;
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
        require_relay(&tx)?;
        let clock = super::workload::capture_control_time(&tx)?;
        if let Some(receipt) = replay(&tx, &input.operation_id, &hash, "prune")? {
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        let now = clock.at();
        let mut value = message(&tx, &input.message_id)?.ok_or(RelayFailure::NotFound)?;
        if timestamp(&value.expires_at)? > now {
            return Err(RelayFailure::Forbidden.into());
        }
        value.body_pruned = true;
        let record = Record {
            version: 1,
            operation_id: input.operation_id.clone(),
            input_hash: hash,
            action: "prune".into(),
            actor: None,
            resource: Resource::Message(value),
        };
        let result = self.persist_relay(&tx, &record, None, &clock, Origin::Request)?;
        self.commit_authorized(tx, &clock)?;
        Ok(result)
    }
}

/// Metadata rule of an `ack` or `prune` fact over the audited message `old`.
/// `actor` is the endpoint the fact is attributed to and `at` the instant of
/// the fact. An acknowledgement comes from the destination, sets
/// `acknowledged` and may also retire a body before expiry.
/// A prune is unattributed and never precedes expiry. Neither can restore a
/// body or change any other field.
fn check_message_update(
    action: &str,
    old: &RelayMessageMetadataV1,
    value: &RelayMessageMetadataV1,
    actor: Option<&RelayEndpointV1>,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut expected = old.clone();
    if action == "ack" {
        expected.acknowledged = true;
        expected.body_pruned = old.body_pruned || value.body_pruned;
        anyhow::ensure!(
            actor == Some(&value.destination),
            "ack actor differs from message destination"
        );
        if !old.body_pruned && value.body_pruned {
            anyhow::ensure!(
                at < timestamp(&old.expires_at)?,
                "expired relay body release on acknowledgement"
            );
        }
    } else {
        expected.body_pruned = true;
        anyhow::ensure!(
            actor.is_none() && timestamp(&value.expires_at)? <= at,
            "premature or worker body prune"
        );
    }
    anyhow::ensure!(
        expected == *value,
        "relay message update changed immutable metadata"
    );
    Ok(())
}

/// Reconstruct every relay projection from metadata-only audit and verify retained bodies by hash.
/// Called by the main control validator, including on external SQLite changes and startup.
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 6 {
        return Ok(());
    }
    let mut grants: BTreeMap<String, (Grant, i64)> = BTreeMap::new();
    let mut messages: BTreeMap<String, (RelayMessageMetadataV1, i64, String, i64)> =
        BTreeMap::new();
    let mut operations = BTreeMap::new();
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = db.prepare(
        "SELECT seq,payload,received_at FROM events WHERE kind GLOB 'control.relay.*' ORDER BY seq",
    )?;
    let events = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (seq, payload, received_at) in events {
        let event: Event = serde_json::from_str(&payload)?;
        let at = timestamp(&event.timestamp)?;
        super::workload::validate_control_time_floor(db, at, Some(seq))?;
        valid(
            canonical_time(timestamp(&received_at)?) == received_at
                && timestamp(&received_at)? >= at,
        )?;
        if event.kind == "control.relay.read" {
            let read: ReadRecord = serde_json::from_value(event.data)?;
            valid(
                read.version == 1
                    && read.limit > 0
                    && read.limit <= 32
                    && read.message_ids.len() <= read.limit as usize
                    && read.after.is_none_or(|v| v >= 0),
            )?;
            read.actor.validate_at(db, &tenant, at, seq)?;
            let mut seen = std::collections::BTreeSet::new();
            for id in &read.message_ids {
                anyhow::ensure!(seen.insert(id), "duplicate message in relay read audit");
                let (message, accepted, _, _) = messages
                    .get(id)
                    .context("relay read references missing message")?;
                let grant = grants
                    .get(&message.grant_id)
                    .context("relay read grant missing")?;
                anyhow::ensure!(
                    message.destination == read.actor.endpoint
                        && !message.body_pruned
                        && !message.acknowledged
                        && timestamp(&message.expires_at)? > at
                        && !grant.0.revoked
                        && timestamp(&grant.0.expires_at)? > at
                        && *accepted > read.after.unwrap_or(0),
                    "relay read differs from authorized message scope"
                );
            }
            continue;
        }
        anyhow::ensure!(
            event.kind == "control.relay.mutation",
            "unknown authoritative relay event"
        );
        let record: Record = serde_json::from_value(event.data)?;
        valid(record.version == 1)?;
        uuid(&record.operation_id)?;
        valid(
            record.input_hash.len() == 64
                && record.input_hash.bytes().all(|v| v.is_ascii_hexdigit()),
        )?;
        anyhow::ensure!(
            operations
                .insert(
                    record.operation_id.clone(),
                    (
                        record.input_hash,
                        record.action.clone(),
                        record.resource.id().to_string(),
                        seq
                    )
                )
                .is_none(),
            "duplicate relay operation in audit"
        );
        match record.resource {
            Resource::Grant(value) => {
                value.source.validate()?;
                value.destination.validate()?;
                uuid(&value.grant_id)?;
                valid(channel_valid(&value.channel))?;
                valid(value.source.worker_id != value.destination.worker_id)?;
                match record.action.as_str() {
                    "grant" => {
                        anyhow::ensure!(
                            !value.revoked && !grants.contains_key(&value.grant_id),
                            "invalid relay grant genesis"
                        );
                        let accepted_at = timestamp(&event.timestamp)?;
                        let expires_at = timestamp(&value.expires_at)?;
                        valid(
                            expires_at > accepted_at
                                && expires_at <= accepted_at + Duration::seconds(MAX_GRANT_SECONDS),
                        )?;
                    }
                    "revoke_grant" => {
                        let mut old = grants
                            .get(&value.grant_id)
                            .context("grant revoke without genesis")?
                            .0
                            .clone();
                        old.revoked = true;
                        anyhow::ensure!(old == value, "grant revoke changed immutable metadata");
                    }
                    _ => anyhow::bail!("invalid relay grant action"),
                }
                anyhow::ensure!(record.actor.is_none(), "worker mutated relay grant");
                grants.insert(value.grant_id.clone(), (value, seq));
            }
            Resource::Message(value) => {
                valid(
                    value.version == 1
                        && value.body_sha256.len() == 64
                        && value
                            .body_sha256
                            .bytes()
                            .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase()),
                )?;
                uuid(&value.message_id)?;
                value.source.validate()?;
                value.destination.validate()?;
                valid(
                    canonical_time(timestamp(&value.created_at)?) == value.created_at
                        && canonical_time(timestamp(&value.expires_at)?) == value.expires_at,
                )?;
                let (accepted, accepted_received_at) = match record.action.as_str() {
                    "send" => {
                        anyhow::ensure!(
                            !messages.contains_key(&value.message_id)
                                && !value.acknowledged
                                && !value.body_pruned
                                && value.message_id == record.operation_id,
                            "invalid relay send genesis"
                        );
                        let actor = record.actor.as_ref().context("send actor missing")?;
                        actor.validate_at(db, &tenant, at, seq)?;
                        anyhow::ensure!(
                            actor.endpoint == value.source,
                            "send actor differs from message source"
                        );
                        let authority =
                            grants.get(&value.grant_id).context("send grant missing")?;
                        anyhow::ensure!(
                            !authority.0.revoked
                                && authority.0.source == value.source
                                && authority.0.destination == value.destination
                                && authority.0.channel == value.channel,
                            "send differs from directed grant"
                        );
                        let start = timestamp(&value.created_at)?;
                        let end = timestamp(&value.expires_at)?;
                        valid(
                            value.body_bytes > 0
                                && value.body_bytes <= MAX_MESSAGE_BYTES as u64
                                && end > start
                                && start <= timestamp(&event.timestamp)?
                                && end
                                    <= start
                                        + Duration::seconds(i64::from(MAX_MESSAGE_TTL_SECONDS))
                                && end <= timestamp(&authority.0.expires_at)?,
                        )?;
                        (seq, received_at.clone())
                    }
                    "ack" | "prune" => {
                        let (old, accepted, accepted_received_at, _) = messages
                            .get(&value.message_id)
                            .context("message update without genesis")?;
                        if record.action == "ack" {
                            let actor = record.actor.as_ref().context("ack actor missing")?;
                            actor.validate_at(db, &tenant, at, seq)?;
                        }
                        check_message_update(
                            &record.action,
                            old,
                            &value,
                            record.actor.as_ref().map(|actor| &actor.endpoint),
                            at,
                        )?;
                        (*accepted, accepted_received_at.clone())
                    }
                    _ => anyhow::bail!("invalid relay message action"),
                };
                messages.insert(
                    value.message_id.clone(),
                    (value, accepted, accepted_received_at, seq),
                );
            }
        }
    }
    let mut stmt =
        db.prepare("SELECT grant_id,payload_json,event_seq FROM relay_grants ORDER BY grant_id")?;
    let actual = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = grants
        .into_iter()
        .map(|(id, (v, seq))| Ok((id, serde_json::to_string(&v)?, seq)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    anyhow::ensure!(
        actual == expected,
        "relay grant projection differs from audit"
    );
    let mut stmt = db.prepare(&format!("{MESSAGE_ROW_SQL} ORDER BY message_id"))?;
    let rows = stmt
        .query_map([], MessageRow::read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        rows.len() == messages.len(),
        "relay message count differs from audit"
    );
    for row in rows {
        let (expected, expected_accepted, expected_received, expected_seq) = messages
            .remove(&row.id)
            .context("unrecorded relay message")?;
        check_message_projection(
            &row,
            &expected,
            expected_accepted,
            &expected_received,
            expected_seq,
        )?;
    }
    let mut stmt = db.prepare("SELECT operation_id,input_hash,action,resource_id,event_seq FROM relay_operations ORDER BY operation_id")?;
    let actual = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = operations
        .into_iter()
        .map(|(id, (hash, action, resource, seq))| (id, hash, action, resource, seq))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        actual == expected,
        "relay operation projection differs from audit"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../relay/query_tests.rs"]
mod query_tests;

#[cfg(test)]
#[path = "relay/scoped_tests.rs"]
mod scoped_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The same construction used by relay_grant and the real relay projection
    /// validator, on a disk SQLite fixture. This is a precision regression, not
    /// a worker authentication or runtime qualification fixture.
    #[test]
    fn grant_canonical_time_rejects_collapsed_interval_before_write_and_reopens_valid_next_ms() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("relay-time.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(super::super::EVENTS_SCHEMA).unwrap();
        db.execute_batch(super::super::BINDING_SCHEMA).unwrap();
        db.execute("INSERT INTO tenant_binding VALUES(1,'tenant-a',1)", [])
            .unwrap();
        initialize(&db).unwrap();
        super::query_tests::install_time_floor(&db);
        db.pragma_update(None, "user_version", 6).unwrap();
        let now = timestamp("2026-11-03T10:00:00.123100Z").unwrap();
        let input = RelayGrantV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            grant_id: uuid::Uuid::new_v4().to_string(),
            source: RelayEndpointV1 {
                worker_id: uuid::Uuid::new_v4().to_string(),
                attempt_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
            },
            destination: RelayEndpointV1 {
                worker_id: uuid::Uuid::new_v4().to_string(),
                attempt_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
            },
            channel: "tasks".into(),
            expires_at: "2026-11-03T10:00:00.123900Z".into(),
        };
        let head = audit_head(&db).unwrap();
        let rejected = grant_record_at(
            &input,
            timestamp(&input.expires_at).unwrap(),
            now,
            input_hash("grant", &input).unwrap(),
        );
        assert!(
            rejected.is_err(),
            "distinct nanosecond instants collapsed into an invalid durable grant"
        );
        assert_eq!(audit_head(&db).unwrap(), head);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM relay_grants", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        validate(&db).unwrap();

        let input = RelayGrantV1 {
            expires_at: "2026-11-03T10:00:00.124000Z".into(),
            ..input
        };
        let record = grant_record_at(
            &input,
            timestamp(&input.expires_at).unwrap(),
            now,
            input_hash("grant", &input).unwrap(),
        )
        .unwrap();
        let mut event = Event::new("control.relay.mutation");
        event.timestamp = canonical_time(now);
        event.data = serde_json::to_value(&record).unwrap();
        let payload = serde_json::to_string(&event).unwrap();
        let hash = super::super::digest(1, &event.timestamp, &head.1, &payload);
        db.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(1,?1,?2,?3,?4,?5,?6)",params![event.id,event.timestamp,head.1,hash,payload,event.kind]).unwrap();
        super::super::workload::advance_time_floor(&db, 1, &event.timestamp).unwrap();
        let Resource::Grant(value) = &record.resource else {
            panic!("grant construction returned another resource")
        };
        db.execute(
            "INSERT INTO relay_grants(grant_id,payload_json,event_seq) VALUES(?1,?2,1)",
            params![value.grant_id, serde_json::to_string(value).unwrap()],
        )
        .unwrap();
        db.execute("INSERT INTO relay_operations(operation_id,input_hash,action,resource_id,event_seq) VALUES(?1,?2,'grant',?3,1)",params![record.operation_id,record.input_hash,value.grant_id]).unwrap();
        validate(&db).unwrap();
        let committed_head = audit_head(&db).unwrap();
        drop(db);
        let reopened = Connection::open(&path).unwrap();
        validate(&reopened).unwrap();
        assert_eq!(audit_head(&reopened).unwrap(), committed_head);
    }

    /// Pure rule of the history validator. These synthetic metadata prove no
    /// worker identity: the validator checks the actor's binding separately.
    #[test]
    fn message_update_rule_never_restores_a_body_and_keeps_release_on_ack_narrow() {
        let endpoint = |worker: &str| RelayEndpointV1 {
            worker_id: worker.into(),
            attempt_id: "00000000-0000-4000-8000-000000000000".into(),
            generation: 1,
        };
        let old = RelayMessageMetadataV1 {
            version: 1,
            message_id: "11111111-1111-4111-8111-111111111111".into(),
            grant_id: "22222222-2222-4222-8222-222222222222".into(),
            source: endpoint("a1a1a1a1-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            destination: endpoint("a2a2a2a2-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            channel: "tasks".into(),
            created_at: "2026-11-10T12:00:00.000Z".into(),
            expires_at: "2026-11-10T12:00:10.000Z".into(),
            body_bytes: 5,
            body_sha256: "0".repeat(64),
            acknowledged: false,
            body_pruned: false,
        };
        let live = timestamp("2026-11-10T12:00:09.999Z").unwrap();
        let expiry = timestamp(&old.expires_at).unwrap();
        let expired = timestamp("2026-11-10T12:00:10.001Z").unwrap();
        let recipient = Some(&old.destination);
        let with = |acknowledged: bool, body_pruned: bool| RelayMessageMetadataV1 {
            acknowledged,
            body_pruned,
            ..old.clone()
        };
        let refused = |result: anyhow::Result<()>, reason: &str, case: &str| {
            let error = result.err().unwrap_or_else(|| panic!("accepted: {case}"));
            assert!(error.to_string().contains(reason), "{case}: {error}");
        };

        // An acknowledgement keeps the body, or retires it in the same fact.
        check_message_update("ack", &old, &with(true, false), recipient, live).unwrap();
        check_message_update("ack", &old, &with(true, true), recipient, live).unwrap();
        for at in [expiry, expired] {
            // Historical acknowledgements that kept the body remain valid.
            check_message_update("ack", &old, &with(true, false), recipient, at).unwrap();
            refused(
                check_message_update("ack", &old, &with(true, true), recipient, at),
                "expired relay body release on acknowledgement",
                "an acknowledgement released a body at or after expiry",
            );
        }
        // Once a body is pruned, no later fact may declare it present again.
        let pruned = with(false, true);
        refused(
            check_message_update("ack", &pruned, &with(true, false), recipient, expiry),
            "changed immutable metadata",
            "an acknowledgement fact restored a pruned body",
        );
        check_message_update("ack", &pruned, &with(true, true), recipient, expiry).unwrap();
        check_message_update("ack", &pruned, &with(true, true), recipient, expired).unwrap();
        // Only the destination acknowledges, and with it releases.
        for actor in [Some(&old.source), None] {
            refused(
                check_message_update("ack", &old, &with(true, true), actor, live),
                "ack actor differs",
                "a body released by an acknowledgement that is not the recipient's",
            );
        }
        // An `ack` fact cannot release without acknowledging, nor touch anything else.
        refused(
            check_message_update("ack", &old, &with(false, true), recipient, live),
            "changed immutable metadata",
            "an ack fact that prunes without acknowledging",
        );
        let moved = RelayMessageMetadataV1 {
            expires_at: "2026-11-10T12:00:20.000Z".into(),
            ..with(true, true)
        };
        refused(
            check_message_update("ack", &old, &moved, recipient, live),
            "changed immutable metadata",
            "an ack fact that moved the expiry",
        );

        // A prune is unattributed and waits for the expiry, inclusive.
        check_message_update("prune", &old, &with(false, true), None, expiry).unwrap();
        refused(
            check_message_update("prune", &old, &with(false, true), None, live),
            "premature or worker body prune",
            "a prune before expiry",
        );
        refused(
            check_message_update("prune", &old, &with(false, true), recipient, expiry),
            "premature or worker body prune",
            "a prune attributed to a worker",
        );
        refused(
            check_message_update("prune", &old, &with(true, true), None, expiry),
            "changed immutable metadata",
            "a prune that also acknowledges",
        );
        refused(
            check_message_update("prune", &old, &with(false, false), None, expiry),
            "changed immutable metadata",
            "a prune that keeps the body",
        );
        // An acknowledged message stays acknowledged when its body is pruned.
        let acknowledged = with(true, false);
        check_message_update("prune", &acknowledged, &with(true, true), None, expiry).unwrap();
        refused(
            check_message_update("prune", &acknowledged, &with(false, true), None, expiry),
            "changed immutable metadata",
            "a prune that cleared the acknowledgement",
        );
    }
}
