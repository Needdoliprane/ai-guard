//! Digest-only workload bindings with indexed historical authorization.
use super::*;
use crate::{
    broker_registry::{BrokerIdentityV1, BrokerV1, format_time, parse_time},
    relay::{RelayEndpointV1, RelayFailure, WorkerRelayIdentity},
    workload::*,
};
use chrono::{DateTime, Utc};

pub(crate) mod projection;
pub(super) use projection::canonical_control_time;
use projection::{Change, Genesis, Projection, ProjectionCell, Record, worker_matches};

/// Proposed owned broker binding writers (IdentityAuthority) and status reads.
pub(super) mod owned_broker_identity;
#[cfg(test)]
#[path = "workload_scoped_tests.rs"]
mod scoped_tests;

pub(super) const SCHEMA: [(&str, &str, &str); 9] = [
    (
        "table",
        "audit_time_floors",
        "CREATE TABLE audit_time_floors(event_seq INTEGER PRIMARY KEY REFERENCES events(seq),max_received_at TEXT NOT NULL)",
    ),
    (
        "table",
        "workload_meta",
        "CREATE TABLE workload_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "workload_bindings",
        "CREATE TABLE workload_bindings(credential_id TEXT PRIMARY KEY,revision INTEGER NOT NULL,sha256_digest TEXT NOT NULL UNIQUE,subject_key TEXT NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "index",
        "workload_bindings_subject",
        "CREATE INDEX workload_bindings_subject ON workload_bindings(subject_key)",
    ),
    (
        "table",
        "workload_binding_history",
        "CREATE TABLE workload_binding_history(credential_id TEXT NOT NULL,revision INTEGER NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),PRIMARY KEY(credential_id,revision))",
    ),
    (
        "index",
        "workload_binding_history_prefix",
        "CREATE INDEX workload_binding_history_prefix ON workload_binding_history(credential_id,event_seq)",
    ),
    (
        "table",
        "workload_digests",
        "CREATE TABLE workload_digests(sha256_digest TEXT PRIMARY KEY,credential_id TEXT NOT NULL REFERENCES workload_bindings(credential_id))",
    ),
    (
        "table",
        "workload_operations",
        "CREATE TABLE workload_operations(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),credential_id TEXT NOT NULL,revision INTEGER NOT NULL)",
    ),
    (
        "table",
        "workload_worker_endpoints",
        "CREATE TABLE workload_worker_endpoints(worker_id TEXT PRIMARY KEY REFERENCES workers(worker_id),attempt_id TEXT NOT NULL UNIQUE REFERENCES worker_attempts(attempt_id),generation INTEGER NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 6] = [
    ("workload_meta", 1),
    ("workload_bindings", 2),
    ("workload_binding_history", 2),
    ("workload_digests", 1),
    ("workload_operations", 2),
    ("workload_worker_endpoints", 3),
];
const QUERIES: [(&str, &str); 6] = [
    (
        "workload_meta",
        "SELECT singleton,genesis_seq,genesis_hash FROM workload_meta ORDER BY singleton",
    ),
    (
        "workload_bindings",
        "SELECT credential_id,revision,sha256_digest,subject_key,payload_json,event_seq FROM workload_bindings ORDER BY credential_id",
    ),
    (
        "workload_binding_history",
        "SELECT credential_id,revision,payload_json,event_seq FROM workload_binding_history ORDER BY credential_id,revision",
    ),
    (
        "workload_digests",
        "SELECT sha256_digest,credential_id FROM workload_digests ORDER BY sha256_digest",
    ),
    (
        "workload_operations",
        "SELECT operation_id,input_hash,event_seq,credential_id,revision FROM workload_operations ORDER BY operation_id",
    ),
    (
        "workload_worker_endpoints",
        "SELECT worker_id,attempt_id,generation,event_seq FROM workload_worker_endpoints ORDER BY worker_id",
    ),
];
fn require_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        Ok(())
    } else {
        Err(WorkloadFailureV1::MigrationRequired.into())
    }
}

fn require_checked_schema(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
    // Format14 is usable only through the actual transaction's borrowed
    // source epoch, never because its version header alone looks modern.
    tx.require_business_access()?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if matches!(version, 14 | 15) {
        Ok(())
    } else {
        require_schema(tx)
    }
}

pub(super) fn validate_control_time(
    db: &Connection,
    received_at: &str,
    before_seq: Option<i64>,
) -> anyhow::Result<()> {
    validate_control_time_floor(db, parse_time(received_at)?, before_seq)
}

/// Snapshot-bound preparation; never serialized or constructed by HTTP input.
#[derive(Debug)]
pub(super) struct AuthorizedControlTimeV1 {
    raw: DateTime<Utc>,
    at: DateTime<Utc>,
}
impl AuthorizedControlTimeV1 {
    pub(super) fn at(&self) -> DateTime<Utc> {
        self.at
    }
}
fn control_time_at(db: &Connection, raw: DateTime<Utc>) -> anyhow::Result<AuthorizedControlTimeV1> {
    let at = canonical_control_time(raw)?;
    validate_control_time_floor(db, at, None)?;
    Ok(AuthorizedControlTimeV1 { raw, at })
}
pub(super) fn capture_control_time(db: &Connection) -> anyhow::Result<AuthorizedControlTimeV1> {
    control_time_at(db, Utc::now())
}
pub(super) fn validate_fresh_control_time(
    db: &Connection,
    authorized: &AuthorizedControlTimeV1,
) -> anyhow::Result<()> {
    validate_fresh_control_time_at(db, authorized, Utc::now())
}
pub(super) fn validate_fresh_control_time_at(
    db: &Connection,
    authorized: &AuthorizedControlTimeV1,
    raw: DateTime<Utc>,
) -> anyhow::Result<()> {
    if raw < authorized.raw {
        return Err(WorkloadFailureV1::ClockRegression.into());
    }
    validate_control_time_floor(db, canonical_control_time(raw)?, None)
}
pub(super) fn validate_control_time_floor(
    db: &Connection,
    at: DateTime<Utc>,
    before_seq: Option<i64>,
) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if ![6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        return Ok(());
    }
    let at = canonical_control_time(at)?;
    let previous: Option<String> = db
        .query_row(
            "SELECT max_received_at FROM audit_time_floors WHERE event_seq<?1 ORDER BY event_seq DESC LIMIT 1",
            [before_seq.unwrap_or(i64::MAX)],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(previous) = previous
        && at < parse_time(&previous)?
    {
        return Err(WorkloadFailureV1::ClockRegression.into());
    }
    Ok(())
}
pub(super) fn time_floors_present(db: &Connection) -> anyhow::Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='audit_time_floors')",[],|row|row.get(0))?)
}
fn seed_time_floors(db: &Connection) -> anyhow::Result<()> {
    let mut statement = db.prepare("SELECT seq,received_at FROM events ORDER BY seq")?;
    let mut maximum: Option<DateTime<Utc>> = None;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (seq, received) = row?;
        let received = canonical_control_time(parse_time(&received)?)?;
        let high = maximum.map_or(received, |previous| previous.max(received));
        maximum = Some(high);
        db.execute(
            "INSERT INTO audit_time_floors(event_seq,max_received_at) VALUES(?1,?2)",
            params![seq, format_time(high)],
        )?;
    }
    Ok(())
}
pub(super) fn advance_time_floor(db: &Connection, seq: i64, received: &str) -> anyhow::Result<()> {
    let previous=db.query_row("SELECT event_seq,max_received_at FROM audit_time_floors WHERE event_seq<?1 ORDER BY event_seq DESC LIMIT 1",[seq],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?))).optional()?;
    let received = canonical_control_time(parse_time(received)?)?;
    let high = if let Some((previous_seq, previous)) = previous {
        anyhow::ensure!(
            previous_seq.checked_add(1) == Some(seq),
            "control time floor prefix is incomplete"
        );
        parse_time(&previous)?.max(received)
    } else {
        anyhow::ensure!(seq == 1, "control time floor prefix is missing");
        received
    };
    db.execute(
        "INSERT INTO audit_time_floors(event_seq,max_received_at) VALUES(?1,?2)",
        params![seq, format_time(high)],
    )?;
    Ok(())
}
pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    seed_time_floors(tx)?;
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new("control.workload.genesis");
    event.data = serde_json::to_value(Genesis {
        version: 1,
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        previous_seq,
        previous_hash,
    })?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    workload_valid(serde_json::to_value(&event)? == original)?;
    let stored = store.append_in_transaction(tx, event, payload)?;
    tx.execute(
        "INSERT INTO workload_meta(singleton,genesis_seq,genesis_hash) VALUES(1,?1,?2)",
        params![stored.seq, stored.hash],
    )?;
    Ok(())
}
fn load_binding(db: &Connection, id: &str) -> anyhow::Result<Option<WorkloadBinding>> {
    db.query_row(
        "SELECT payload_json FROM workload_bindings WHERE credential_id=?1",
        [id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}
pub(super) fn prefix_binding(
    db: &Connection,
    id: &str,
    before_seq: i64,
) -> anyhow::Result<Option<WorkloadBinding>> {
    // The compound index seeks directly to this credential's prefix. Work is
    // independent of other credentials and later rotations of this credential.
    db.query_row("SELECT payload_json FROM workload_binding_history WHERE credential_id=?1 AND event_seq<?2 ORDER BY event_seq DESC LIMIT 1",params![id,before_seq],|row|row.get::<_,String>(0)).optional()?
        .map(|json|serde_json::from_str(&json).map_err(Into::into)).transpose()
}
fn worker(db: &Connection, id: &str) -> anyhow::Result<Option<crate::control::WorkerV1>> {
    db.query_row(
        "SELECT snapshot_json FROM workers WHERE worker_id=?1",
        [id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}
fn worker_relay_matches(worker: &crate::control::WorkerV1, endpoint: &RelayEndpointV1) -> bool {
    worker_matches(worker, endpoint)
        && endpoint.generation == 1
        && worker.state == crate::control::WorkerStateV1::Running
        && worker.attempt.container_id.is_some()
        && worker.attempt.created_inspection.is_some()
        && !worker.allocation_inconsistent
}
fn validate_issue_worker(
    db: &Connection,
    change: &Change,
    before_seq: Option<i64>,
) -> anyhow::Result<()> {
    let Some(endpoint) = change.worker_issue_endpoint() else {
        return Ok(());
    };
    let worker = if let Some(before_seq) = before_seq {
        super::control::worker_at(db, &endpoint.worker_id, before_seq)?
    } else {
        worker(db, &endpoint.worker_id)?
    };
    workload_authorized(
        worker
            .as_ref()
            .is_some_and(|worker| worker_matches(worker, &endpoint)),
    )
}
fn preflight(db: &Connection, change: &Change) -> anyhow::Result<Projection> {
    let mut projection = Projection::default();
    if let Some(binding) = load_binding(db, change.credential_id())? {
        projection
            .bindings
            .insert(binding.credential_id.clone(), (binding, 0));
    }
    if let Change::Issue { input } = change {
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM workload_bindings WHERE subject_key=?1",
            [input.subject.key()?],
            |row| row.get(0),
        )?;
        let count =
            u64::try_from(count).context("invalid negative workload subject binding count")?;
        projection
            .subject_counts
            .insert(input.subject.key()?, count);
        if let Some(owner) = db
            .query_row(
                "SELECT credential_id FROM workload_digests WHERE sha256_digest=?1",
                [&input.sha256_digest],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            projection
                .digests
                .insert(input.sha256_digest.clone(), owner);
        }
        if let WorkloadSubjectV1::Worker { worker_id, .. } = &input.subject
            && let Some((attempt,generation,seq))=db.query_row("SELECT attempt_id,generation,event_seq FROM workload_worker_endpoints WHERE worker_id=?1",[worker_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?))).optional()? {
                let generation = u64::try_from(generation).context("invalid negative workload endpoint generation")?;
                projection.endpoints.insert(worker_id.clone(),(RelayEndpointV1{worker_id:worker_id.clone(),attempt_id:attempt,generation},seq));
            }
    }
    Ok(projection)
}
pub(crate) fn receipt(
    stored: &StoredEvent,
    credential_id: String,
    revision: u64,
    replayed: bool,
) -> WorkloadBindingReceiptV1 {
    WorkloadBindingReceiptV1 {
        version: 1,
        credential_id,
        revision,
        audit_event_id: stored.event.id.clone(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
        replayed,
    }
}
fn replay(db: &Connection, change: &Change) -> anyhow::Result<Option<WorkloadBindingReceiptV1>> {
    let row=db.query_row("SELECT input_hash,event_seq,credential_id,revision FROM workload_operations WHERE operation_id=?1",[change.operation_id()],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?))).optional()?;
    let Some((hash, seq, id, revision)) = row else {
        return Ok(None);
    };
    let revision = u64::try_from(revision).context("invalid negative workload binding revision")?;
    workload_conflict(hash == change.input_hash()?)?;
    let stored = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [seq],
        row_event,
    )?;
    workload_conflict(stored.event.kind == change.kind())?;
    Ok(Some(receipt(&stored, id, revision, true)))
}

fn workload_digest_binding(
    db: &Connection,
    digest: &[u8; 32],
) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 6 {
        return Ok(None);
    }
    require_schema(db)?;
    load_digest_binding(db, digest)
}

fn load_digest_binding(
    db: &Connection,
    digest: &[u8; 32],
) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
    let owner = db.query_row("SELECT d.credential_id,b.payload_json FROM workload_digests d JOIN workload_bindings b ON b.credential_id=d.credential_id WHERE d.sha256_digest=?1", [hex::encode(digest)], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
    owner
        .map(
            |(credential_id, json)| -> anyhow::Result<WorkloadDigestOwnerV1> {
                let binding: WorkloadBinding = serde_json::from_str(&json)?;
                workload_authorized(credential_id == binding.credential_id)?;
                Ok(WorkloadDigestOwnerV1 {
                    credential_id,
                    subject: binding.subject,
                })
            },
        )
        .transpose()
}

impl Store {
    /// Resolve immutable historical ownership, including rotated and revoked
    /// digests. An absent owner is returned only from a healthy checked read.
    pub(crate) fn workload_digest_binding(
        &self,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
        self.workload_digest_binding_with_access(None, digest)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn workload_digest_binding_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
        self.workload_digest_binding_with_access(Some(access), digest)
    }
    fn workload_digest_binding_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
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
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let owner = if version < 6 {
            None
        } else {
            require_checked_schema(&tx)?;
            load_digest_binding(&tx, digest)?
        };
        self.commit_checked(tx)?;
        Ok(owner)
    }
    pub(crate) fn try_workload_digest_binding(
        &self,
        digest: &[u8; 32],
    ) -> MetadataLookup<WorkloadDigestOwnerV1> {
        self.with_known_binding(
            self.try_read_metadata(|db| workload_digest_binding(db, digest)),
            |bindings| bindings.workload.get(&hex::encode(digest)).cloned(),
        )
    }
    pub fn issue_workload_binding(
        &self,
        input: &WorkloadBindingIssueV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(
            None,
            Change::Issue {
                input: input.clone(),
            },
        )
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn issue_workload_binding_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        input: &WorkloadBindingIssueV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(
            Some(access),
            Change::Issue {
                input: input.clone(),
            },
        )
    }
    pub fn revoke_workload_binding(
        &self,
        input: &WorkloadBindingRevocationV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(
            None,
            Change::Revoke {
                input: input.clone(),
            },
        )
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn revoke_workload_binding_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        input: &WorkloadBindingRevocationV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(
            Some(access),
            Change::Revoke {
                input: input.clone(),
            },
        )
    }
    fn persist_workload(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        change: Change,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        change.validate()?;
        let value = serde_json::to_value(&change)?;
        workload_valid(self.redactor.value(&value) == value)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let mut tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        require_checked_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        let receipt = self.persist_workload_checked(&mut tx, &clock, change)?;
        self.commit_authorized(tx, &clock)?;
        Ok(receipt)
    }
    /// Shared binding mutation body over a checked transaction whose access
    /// class, format guard and control clock the caller already established.
    /// An exact replay returns the original receipt without a second write.
    /// The caller alone commits; nothing is published before that commit.
    fn persist_workload_checked(
        &self,
        tx: &mut CheckedTransaction<'_>,
        clock: &AuthorizedControlTimeV1,
        change: Change,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        if let Some(receipt) = replay(tx, &change)? {
            return Ok(receipt);
        }
        validate_issue_worker(tx, &change, None)?;
        let record = Record {
            version: 1,
            input_hash: change.input_hash()?,
            received_at: format_time(clock.at()),
            change,
        };
        validate_control_time(tx, &record.received_at, None)?;
        let mut projection = preflight(tx, &record.change)?;
        let next_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let binding = projection.apply(&record, next_seq)?;
        let mut event = Event::new(record.change.kind());
        event.timestamp = record.received_at.clone();
        event.data = serde_json::to_value(&record)?;
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        workload_valid(serde_json::to_value(&event)? == original)?;
        let stored = if matches!(record.change, Change::Revoke { .. }) {
            self.append_authorized_identity_in_transaction(tx, event, payload, clock)?
        } else {
            self.append_authorized_in_transaction(tx, event, payload, clock)?
        };
        workload_valid(
            stored.seq == next_seq
                && parse_time(&record.received_at)? <= parse_time(&stored.received_at)?,
        )?;
        tx.execute("INSERT INTO workload_bindings(credential_id,revision,sha256_digest,subject_key,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(credential_id) DO UPDATE SET revision=excluded.revision,sha256_digest=excluded.sha256_digest,subject_key=excluded.subject_key,payload_json=excluded.payload_json,event_seq=excluded.event_seq",params![binding.credential_id,i64::try_from(binding.revision)?,binding.sha256_digest,binding.subject.key()?,serde_json::to_string(&binding)?,stored.seq])?;
        tx.execute("INSERT INTO workload_binding_history(credential_id,revision,payload_json,event_seq) VALUES(?1,?2,?3,?4)",params![binding.credential_id,i64::try_from(binding.revision)?,serde_json::to_string(&binding)?,stored.seq])?;
        if let Change::Issue { .. } = &record.change {
            tx.execute("INSERT INTO workload_digests(sha256_digest,credential_id) VALUES(?1,?2) ON CONFLICT(sha256_digest) DO NOTHING",params![binding.sha256_digest,binding.credential_id])?;
            if let WorkloadSubjectV1::Worker {
                worker_id,
                attempt_id,
                generation,
            } = &binding.subject
            {
                tx.execute("INSERT INTO workload_worker_endpoints(worker_id,attempt_id,generation,event_seq) VALUES(?1,?2,?3,?4) ON CONFLICT(worker_id) DO NOTHING",params![worker_id,attempt_id,i64::try_from(*generation)?,stored.seq])?;
            }
        }
        tx.execute("INSERT INTO workload_operations(operation_id,input_hash,event_seq,credential_id,revision) VALUES(?1,?2,?3,?4,?5)",params![record.change.operation_id(),record.input_hash,stored.seq,binding.credential_id,i64::try_from(binding.revision)?])?;
        tx.positive_bindings = Some(metadata::PositiveBindings::load(tx)?);
        Ok(receipt(
            &stored,
            binding.credential_id,
            binding.revision,
            false,
        ))
    }
    pub(crate) fn authenticate_workload_token(
        &self,
        token: &str,
    ) -> anyhow::Result<Option<AuthenticatedWorkload>> {
        self.authenticate_workload_token_with_access(None, token)
    }
    pub(crate) fn authenticate_workload_token_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        token: &str,
    ) -> anyhow::Result<Option<AuthenticatedWorkload>> {
        self.authenticate_workload_token_with_access(Some(access), token)
    }
    fn authenticate_workload_token_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        token: &str,
    ) -> anyhow::Result<Option<AuthenticatedWorkload>> {
        workload_authorized(
            (32..=512).contains(&token.len()) && !token.chars().any(char::is_whitespace),
        )?;
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
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
        require_checked_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        let identity = self.authenticate_workload_digest_checked(&tx, &digest, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(identity)
    }
    /// Shared workload authentication body over a checked transaction whose
    /// access class, format guard and control clock the caller established:
    /// binding, expiry, grant, incarnation and time floor of the digest's
    /// subject. `None` is an unbound digest. The caller alone commits.
    fn authenticate_workload_digest_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        digest: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<AuthenticatedWorkload>> {
        let json = tx
            .query_row(
                "SELECT payload_json FROM workload_bindings WHERE sha256_digest=?1",
                [digest],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(json) = json else {
            return Ok(None);
        };
        let binding: WorkloadBinding = serde_json::from_str(&json)?;
        binding_active(&binding, now)?;
        let identity = match &binding.subject {
            WorkloadSubjectV1::Broker {
                broker_id,
                incarnation_id,
            } => {
                let broker = load_broker(tx, broker_id)?.ok_or(WorkloadFailureV1::Unauthorized)?;
                workload_authorized(broker.incarnation_id == *incarnation_id)?;
                let identity = BrokerIdentityV1::authenticated(
                    self.tenant_id.clone(),
                    broker_id.clone(),
                    binding.credential_id.clone(),
                    binding.revision,
                    broker.grant_revision,
                )?;
                revalidate_broker_identity_checked(tx, &identity, now)?;
                broker.require_current_grant(&identity, &format_time(now))?;
                AuthenticatedWorkload::Broker(identity)
            }
            WorkloadSubjectV1::Worker {
                worker_id,
                attempt_id,
                generation,
            } => {
                let identity = WorkerRelayIdentity {
                    tenant_id: self.tenant_id.clone(),
                    credential_id: binding.credential_id.clone(),
                    worker_id: worker_id.clone(),
                    attempt_id: attempt_id.clone(),
                    generation: *generation,
                    binding_revision: binding.revision,
                };
                self.checked_worker_relay_identity(tx, &identity, now)?;
                AuthenticatedWorkload::Worker(identity)
            }
        };
        Ok(Some(identity))
    }
    pub(super) fn checked_worker_relay_identity(
        &self,
        db: &CheckedTransaction<'_>,
        identity: &WorkerRelayIdentity,
        now: DateTime<Utc>,
    ) -> anyhow::Result<RelayEndpointV1> {
        let check = (|| {
            validate_control_time_floor(db, now, None)?;
            workload_authorized(db.lease.active)?;
            tenant_matches(db, &identity.tenant_id)?;
            let binding = load_binding(db, &identity.credential_id)?
                .ok_or(WorkloadFailureV1::Unauthorized)?;
            validate_worker_binding(&binding, identity, now)?;
            let endpoint = RelayEndpointV1 {
                worker_id: identity.worker_id.clone(),
                attempt_id: identity.attempt_id.clone(),
                generation: identity.generation,
            };
            self.checked_worker_relay_endpoint(db, &endpoint, now)?;
            Ok(endpoint)
        })();
        relay_authorization(check)
    }
    pub(super) fn checked_worker_relay_endpoint(
        &self,
        db: &CheckedTransaction<'_>,
        endpoint: &RelayEndpointV1,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let check = (|| {
            validate_control_time_floor(db, now, None)?;
            workload_authorized(db.lease.active)?;
            endpoint.validate()?;
            tenant_matches(db, &self.tenant_id)?;
            let canonical=db.query_row("SELECT attempt_id,generation FROM workload_worker_endpoints WHERE worker_id=?1",[&endpoint.worker_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).optional()?;
            let canonical = canonical
                .map(|(attempt, generation)| -> anyhow::Result<_> {
                    Ok((
                        attempt,
                        u64::try_from(generation)
                            .context("invalid negative workload endpoint generation")?,
                    ))
                })
                .transpose()?;
            workload_authorized(
                canonical == Some((endpoint.attempt_id.clone(), endpoint.generation)),
            )?;
            workload_authorized(
                worker(db, &endpoint.worker_id)?
                    .as_ref()
                    .is_some_and(|worker| worker_relay_matches(worker, endpoint)),
            )?;
            let subject = WorkloadSubjectV1::Worker {
                worker_id: endpoint.worker_id.clone(),
                attempt_id: endpoint.attempt_id.clone(),
                generation: endpoint.generation,
            };
            let mut statement = db.prepare(
                "SELECT payload_json FROM workload_bindings WHERE subject_key=?1 LIMIT 65",
            )?;
            let rows = statement
                .query_map([subject.key()?], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            workload_authorized(rows.len() <= 64)?;
            let mut active = false;
            for json in rows {
                let binding: WorkloadBinding = serde_json::from_str(&json)?;
                // Parse/corruption failures propagate; only a well-formed
                // expired/revoked credential is an unavailable identity.
                if binding.subject == subject
                    && !binding.revoked
                    && now >= parse_time(&binding.updated_at)?
                    && now < parse_time(&binding.expires_at)?
                {
                    active = true;
                }
            }
            workload_authorized(active)
        })();
        relay_authorization(check)
    }
}
fn relay_authorization<T>(result: anyhow::Result<T>) -> anyhow::Result<T> {
    result.map_err(|error| {
        if error.downcast_ref::<WorkloadFailureV1>() == Some(&WorkloadFailureV1::ClockRegression) {
            return error;
        }
        if error.downcast_ref::<WorkloadFailureV1>().is_some()
            || error
                .downcast_ref::<crate::control::ControlFailure>()
                .is_some()
        {
            RelayFailure::Forbidden.into()
        } else {
            error
        }
    })
}
fn tenant_matches(db: &Connection, tenant: &str) -> anyhow::Result<()> {
    let actual: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    workload_authorized(actual == tenant)
}
fn binding_active(binding: &WorkloadBinding, at: DateTime<Utc>) -> anyhow::Result<()> {
    workload_authorized(binding_is_active(binding, at)?)
}
fn binding_is_active(binding: &WorkloadBinding, at: DateTime<Utc>) -> anyhow::Result<bool> {
    Ok(!binding.revoked
        && at >= parse_time(&binding.updated_at)?
        && at < parse_time(&binding.expires_at)?)
}

/// Checks the grant's workload topology for a new operation in the caller's transaction.
pub(super) fn broker_grant_binding_is_current(
    db: &Connection,
    broker: &BrokerV1,
    at: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let Some(binding) = load_binding(db, &broker.credential_id)? else {
        return Ok(false);
    };
    Ok(binding.credential_id == broker.credential_id
        && matches!(
            &binding.subject,
            WorkloadSubjectV1::Broker { broker_id, incarnation_id }
                if broker_id == &broker.broker_id && incarnation_id == &broker.incarnation_id
        )
        && binding_is_active(&binding, at)?)
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
fn validate_broker_binding(
    binding: &WorkloadBinding,
    identity: &BrokerIdentityV1,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    binding_active(binding, now)?;
    workload_authorized(
        binding.revision == identity.binding_revision()
            && binding.credential_id == identity.credential_id(),
    )?;
    let WorkloadSubjectV1::Broker {
        broker_id,
        incarnation_id,
    } = &binding.subject
    else {
        return Err(WorkloadFailureV1::Unauthorized.into());
    };
    workload_authorized(broker_id == identity.broker_id())?;
    // Incarnation is cross-checked with the grant in revalidation below.
    crate::broker_registry::registry_uuid(incarnation_id)
}
pub(super) fn revalidate_broker_identity(
    db: &Connection,
    identity: &BrokerIdentityV1,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    require_schema(db)?;
    revalidate_broker_identity_state(db, identity, now)
}

pub(super) fn revalidate_broker_identity_checked(
    tx: &CheckedTransaction<'_>,
    identity: &BrokerIdentityV1,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    require_checked_schema(tx)?;
    revalidate_broker_identity_state(tx, identity, now)
}

fn revalidate_broker_identity_state(
    db: &Connection,
    identity: &BrokerIdentityV1,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    validate_control_time_floor(db, now, None)?;
    tenant_matches(db, identity.tenant_id())?;
    let binding =
        load_binding(db, identity.credential_id())?.ok_or(WorkloadFailureV1::Unauthorized)?;
    validate_broker_binding(&binding, identity, now)?;
    let broker = load_broker(db, identity.broker_id())?.ok_or(WorkloadFailureV1::Unauthorized)?;
    if let WorkloadSubjectV1::Broker { incarnation_id, .. } = &binding.subject {
        workload_authorized(broker.incarnation_id == *incarnation_id)?;
    }
    broker.require_current_grant(identity, &format_time(now))
}
pub(super) fn validate_broker_identity_at(
    db: &Connection,
    identity: &BrokerIdentityV1,
    at: DateTime<Utc>,
    before_seq: i64,
) -> anyhow::Result<()> {
    validate_control_time_floor(db, at, Some(before_seq))?;
    tenant_matches(db, identity.tenant_id())?;
    let binding = prefix_binding(db, identity.credential_id(), before_seq)?
        .ok_or(WorkloadFailureV1::Unauthorized)?;
    validate_broker_binding(&binding, identity, at)?;
    let broker = super::broker_registry::broker_at(db, identity.broker_id(), before_seq)?
        .ok_or(WorkloadFailureV1::Unauthorized)?;
    if let WorkloadSubjectV1::Broker { incarnation_id, .. } = &binding.subject {
        workload_authorized(broker.incarnation_id == *incarnation_id)?;
    }
    broker.require_current_grant(identity, &format_time(at))
}
fn validate_worker_binding(
    binding: &WorkloadBinding,
    identity: &WorkerRelayIdentity,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    binding_active(binding, at)?;
    workload_authorized(
        binding.credential_id == identity.credential_id
            && binding.revision == identity.binding_revision
            && binding.subject
                == WorkloadSubjectV1::Worker {
                    worker_id: identity.worker_id.clone(),
                    attempt_id: identity.attempt_id.clone(),
                    generation: identity.generation,
                },
    )
}
pub(super) fn validate_worker_relay_identity_at(
    db: &Connection,
    identity: &WorkerRelayIdentity,
    at: DateTime<Utc>,
    before_seq: i64,
) -> anyhow::Result<()> {
    validate_control_time_floor(db, at, Some(before_seq))?;
    tenant_matches(db, &identity.tenant_id)?;
    let binding = prefix_binding(db, &identity.credential_id, before_seq)?
        .ok_or(WorkloadFailureV1::Unauthorized)?;
    validate_worker_binding(&binding, identity, at)?;
    let worker = super::control::worker_at(db, &identity.worker_id, before_seq)?;
    let endpoint = RelayEndpointV1 {
        worker_id: identity.worker_id.clone(),
        attempt_id: identity.attempt_id.clone(),
        generation: identity.generation,
    };
    workload_authorized(
        worker
            .as_ref()
            .is_some_and(|worker| worker_relay_matches(worker, &endpoint)),
    )
}

type SqlRow = Vec<rusqlite::types::Value>;
impl From<ProjectionCell> for rusqlite::types::Value {
    fn from(value: ProjectionCell) -> Self {
        match value {
            ProjectionCell::Integer(value) => Self::Integer(value),
            ProjectionCell::Text(value) => Self::Text(value),
        }
    }
}

pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if ![6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        return Ok(());
    }
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let (genesis_seq, genesis_hash): (i64, String) = db.query_row(
        "SELECT genesis_seq,genesis_hash FROM workload_meta WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut projection = Projection::default();
    let mut seen = false;
    let mut statement=db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.workload.*' ORDER BY seq")?;
    for stored in statement.query_map([], row_event)? {
        let stored = stored?;
        workload_valid(
            stored.event.source == "gateway"
                && stored.event.provenance == "observed"
                && stored.event.coverage == "observed",
        )?;
        if stored.event.kind == "control.workload.genesis" {
            let genesis: Genesis = serde_json::from_value(stored.event.data.clone())?;
            workload_valid(
                !seen
                    && stored.seq == genesis_seq
                    && stored.hash == genesis_hash
                    && genesis.matches_prefix(&tenant, &namespace, stored.seq, &stored.prev_hash),
            )?;
            seen = true;
            continue;
        }
        workload_valid(seen)?;
        let record: Record = serde_json::from_value(stored.event.data.clone())?;
        workload_valid(
            stored.event.kind == record.change.kind()
                && parse_time(&record.received_at)? <= parse_time(&stored.received_at)?,
        )?;
        validate_control_time(db, &record.received_at, Some(stored.seq))?;
        validate_issue_worker(db, &record.change, Some(stored.seq))?;
        projection.apply(&record, stored.seq)?;
    }
    workload_valid(seen)?;
    let mut expected = projection.rows(genesis_seq, &genesis_hash)?;
    for (table, query) in QUERIES {
        let mut statement = db.prepare(query)?;
        let width = statement.column_count();
        let actual = statement
            .query_map([], |row| {
                (0..width)
                    .map(|col| row.get::<_, rusqlite::types::Value>(col))
                    .collect::<rusqlite::Result<SqlRow>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let expected_rows = expected.remove(table).map(|rows| {
            rows.into_iter()
                .map(|row| row.into_iter().map(rusqlite::types::Value::from).collect())
                .collect()
        });
        anyhow::ensure!(
            expected_rows == Some(actual),
            "workload projection differs from audit: {table}"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "../workload/store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../workload/time_floor_tests.rs"]
mod time_floor_tests;
