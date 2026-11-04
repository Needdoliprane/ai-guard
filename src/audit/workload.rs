//! Digest-only workload bindings with indexed historical authorization.
use super::*;
use crate::{
    broker_registry::{BrokerIdentityV1, BrokerV1, format_time, parse_time},
    relay::{RelayEndpointV1, RelayFailure, WorkerRelayIdentity},
    workload::*,
};
use chrono::{DateTime, Utc};

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
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    tenant_id: String,
    control_namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Issue { input: WorkloadBindingIssueV1 },
    Revoke { input: WorkloadBindingRevocationV1 },
}
impl Change {
    fn operation_id(&self) -> &str {
        match self {
            Self::Issue { input } => &input.operation_id,
            Self::Revoke { input } => &input.operation_id,
        }
    }
    fn credential_id(&self) -> &str {
        match self {
            Self::Issue { input } => &input.credential_id,
            Self::Revoke { input } => &input.credential_id,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Issue { .. } => "control.workload.issue",
            Self::Revoke { .. } => "control.workload.revoke",
        }
    }
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Issue { input } => input.validate(),
            Self::Revoke { input } => input.validate(),
        }
    }
    fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::content_hash("ai-guard:workload-binding-input:v1", self)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    input_hash: String,
    received_at: String,
    change: Change,
}
#[derive(Default)]
struct Projection {
    bindings: BTreeMap<String, (WorkloadBinding, i64)>,
    history: BTreeMap<(String, u64), (WorkloadBinding, i64)>,
    digests: BTreeMap<String, String>,
    operations: BTreeMap<String, (String, i64, String, u64)>,
    endpoints: BTreeMap<String, (RelayEndpointV1, i64)>,
    subject_counts: BTreeMap<String, u64>,
}
impl Projection {
    fn apply(&mut self, record: &Record, seq: i64) -> anyhow::Result<WorkloadBinding> {
        record.change.validate()?;
        workload_valid(record.version == 1 && record.input_hash == record.change.input_hash()?)?;
        workload_conflict(!self.operations.contains_key(record.change.operation_id()))?;
        let now = parse_time(&record.received_at)?;
        let previous = self
            .bindings
            .get(record.change.credential_id())
            .map(|(binding, _)| binding);
        if let Some(previous) = previous {
            if now < parse_time(&previous.updated_at)? {
                return Err(WorkloadFailureV1::ClockRegression.into());
            }
        }
        let binding = match &record.change {
            Change::Issue { input } => {
                let expires_at = canonical_control_time(parse_time(&input.expires_at)?)?;
                workload_valid(expires_at > now)?;
                workload_conflict(
                    input.expected_revision == previous.map_or(0, |binding| binding.revision),
                )?;
                if let Some(previous) = previous {
                    workload_conflict(previous.subject == input.subject)?;
                }
                if previous.is_none() {
                    let count = self.subject_counts.entry(input.subject.key()?).or_default();
                    workload_conflict(*count < 64)?;
                    *count += 1;
                }
                if let Some(owner) = self.digests.get(&input.sha256_digest) {
                    workload_conflict(owner == &input.credential_id)?;
                    // A historical digest is ownership evidence, not a fresh
                    // credential. Only renewal of the still-current active key
                    // may reuse it; rotation and revocation require a new key.
                    let current = previous.ok_or(WorkloadFailureV1::Conflict)?;
                    workload_conflict(
                        !current.revoked
                            && current.sha256_digest == input.sha256_digest
                            && now < parse_time(&current.expires_at)?,
                    )?;
                }
                if let WorkloadSubjectV1::Worker {
                    worker_id,
                    attempt_id,
                    generation,
                } = &input.subject
                {
                    let endpoint = RelayEndpointV1 {
                        worker_id: worker_id.clone(),
                        attempt_id: attempt_id.clone(),
                        generation: *generation,
                    };
                    if let Some((previous, _)) = self.endpoints.get(worker_id) {
                        workload_conflict(previous == &endpoint)?;
                    } else {
                        self.endpoints.insert(worker_id.clone(), (endpoint, seq));
                    }
                }
                let binding = WorkloadBinding {
                    version: 1,
                    credential_id: input.credential_id.clone(),
                    revision: crate::broker_registry::next_revision(input.expected_revision)?,
                    subject: input.subject.clone(),
                    sha256_digest: input.sha256_digest.clone(),
                    expires_at: format_time(expires_at),
                    revoked: false,
                    updated_at: record.received_at.clone(),
                };
                self.digests
                    .insert(input.sha256_digest.clone(), input.credential_id.clone());
                binding
            }
            Change::Revoke { input } => {
                let mut binding = previous.cloned().ok_or(WorkloadFailureV1::Conflict)?;
                workload_conflict(input.expected_revision == binding.revision && !binding.revoked)?;
                binding.revision = crate::broker_registry::next_revision(binding.revision)?;
                binding.revoked = true;
                binding.updated_at = record.received_at.clone();
                binding
            }
        };
        self.bindings
            .insert(binding.credential_id.clone(), (binding.clone(), seq));
        self.history.insert(
            (binding.credential_id.clone(), binding.revision),
            (binding.clone(), seq),
        );
        self.operations.insert(
            record.change.operation_id().into(),
            (
                record.input_hash.clone(),
                seq,
                binding.credential_id.clone(),
                binding.revision,
            ),
        );
        Ok(binding)
    }
}
fn require_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [6, 7].contains(&version) {
        Ok(())
    } else {
        Err(WorkloadFailureV1::MigrationRequired.into())
    }
}

pub(super) fn validate_control_time(
    db: &Connection,
    received_at: &str,
    before_seq: Option<i64>,
) -> anyhow::Result<()> {
    validate_control_time_floor(db, parse_time(received_at)?, before_seq)
}

/// Authorization/expiry use millisecond UTC, matching their durable records.
/// This does not claim detection of a cross-request rollback within one ms.
pub(super) fn canonical_control_time(at: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&format_time(at))
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| WorkloadFailureV1::ClockRegression.into())
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
    if ![6, 7].contains(&version) {
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
    if let Some(previous) = previous {
        if at < parse_time(&previous)? {
            return Err(WorkloadFailureV1::ClockRegression.into());
        }
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
fn prefix_binding(
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
fn worker_matches(worker: &crate::control::WorkerV1, endpoint: &RelayEndpointV1) -> bool {
    worker.worker_id == endpoint.worker_id
        && worker.attempt.attempt_id == endpoint.attempt_id
        && worker.quota_held
        && !matches!(
            worker.state,
            crate::control::WorkerStateV1::Terminal | crate::control::WorkerStateV1::Cancelled
        )
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
    if let Change::Issue { input } = change {
        if let WorkloadSubjectV1::Worker {
            worker_id,
            attempt_id,
            generation,
        } = &input.subject
        {
            let endpoint = RelayEndpointV1 {
                worker_id: worker_id.clone(),
                attempt_id: attempt_id.clone(),
                generation: *generation,
            };
            let worker = if let Some(before_seq) = before_seq {
                super::control::worker_at(db, worker_id, before_seq)?
            } else {
                worker(db, worker_id)?
            };
            workload_authorized(
                worker
                    .as_ref()
                    .is_some_and(|worker| worker_matches(worker, &endpoint)),
            )?;
        }
    }
    Ok(())
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
        if let WorkloadSubjectV1::Worker { worker_id, .. } = &input.subject {
            if let Some((attempt,generation,seq))=db.query_row("SELECT attempt_id,generation,event_seq FROM workload_worker_endpoints WHERE worker_id=?1",[worker_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?))).optional()? {
                let generation = u64::try_from(generation).context("invalid negative workload endpoint generation")?;
                projection.endpoints.insert(worker_id.clone(),(RelayEndpointV1{worker_id:worker_id.clone(),attempt_id:attempt,generation},seq));
            }
        }
    }
    Ok(projection)
}
fn receipt(
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

impl Store {
    /// Resolve immutable historical ownership, including rotated and revoked
    /// digests. An absent owner is returned only from a healthy checked read.
    pub(crate) fn workload_digest_binding(
        &self,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 6 {
            self.commit_checked(tx)?;
            return Ok(None);
        }
        require_schema(&tx)?;
        let owner = tx.query_row("SELECT d.credential_id,b.payload_json FROM workload_digests d JOIN workload_bindings b ON b.credential_id=d.credential_id WHERE d.sha256_digest=?1", [hex::encode(digest)], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
        let owner = owner
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
            .transpose()?;
        self.commit_checked(tx)?;
        Ok(owner)
    }
    pub fn issue_workload_binding(
        &self,
        input: &WorkloadBindingIssueV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(Change::Issue {
            input: input.clone(),
        })
    }
    pub fn revoke_workload_binding(
        &self,
        input: &WorkloadBindingRevocationV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.persist_workload(Change::Revoke {
            input: input.clone(),
        })
    }
    fn persist_workload(&self, change: Change) -> anyhow::Result<WorkloadBindingReceiptV1> {
        change.validate()?;
        let value = serde_json::to_value(&change)?;
        workload_valid(self.redactor.value(&value) == value)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        if let Some(receipt) = replay(&tx, &change)? {
            self.commit_authorized(tx, &clock)?;
            return Ok(receipt);
        }
        validate_issue_worker(&tx, &change, None)?;
        let record = Record {
            version: 1,
            input_hash: change.input_hash()?,
            received_at: format_time(clock.at()),
            change,
        };
        validate_control_time(&tx, &record.received_at, None)?;
        let mut projection = preflight(&tx, &record.change)?;
        let next_seq = audit_head(&tx)?
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
            self.append_authorized_identity_in_transaction(&tx, event, payload, &clock)?
        } else {
            self.append_authorized_in_transaction(&tx, event, payload, &clock)?
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
        self.commit_authorized(tx, &clock)?;
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
        workload_authorized(
            (32..=512).contains(&token.len()) && !token.chars().any(char::is_whitespace),
        )?;
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        let now = clock.at();
        let json = tx
            .query_row(
                "SELECT payload_json FROM workload_bindings WHERE sha256_digest=?1",
                [digest],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(json) = json else {
            self.commit_authorized(tx, &clock)?;
            return Ok(None);
        };
        let binding: WorkloadBinding = serde_json::from_str(&json)?;
        binding_active(&binding, now)?;
        let identity = match &binding.subject {
            WorkloadSubjectV1::Broker {
                broker_id,
                incarnation_id,
            } => {
                let broker = load_broker(&tx, broker_id)?.ok_or(WorkloadFailureV1::Unauthorized)?;
                workload_authorized(broker.incarnation_id == *incarnation_id)?;
                let identity = BrokerIdentityV1::authenticated(
                    self.tenant_id.clone(),
                    broker_id.clone(),
                    binding.credential_id.clone(),
                    binding.revision,
                    broker.grant_revision,
                )?;
                revalidate_broker_identity(&tx, &identity, now)?;
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
                self.checked_worker_relay_identity(&tx, &identity, now)?;
                AuthenticatedWorkload::Worker(identity)
            }
        };
        self.commit_authorized(tx, &clock)?;
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
    workload_authorized(
        !binding.revoked
            && at >= parse_time(&binding.updated_at)?
            && at < parse_time(&binding.expires_at)?,
    )
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
fn sql_text(value: impl Into<String>) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.into())
}
fn sql_int(value: u64) -> anyhow::Result<rusqlite::types::Value> {
    Ok(rusqlite::types::Value::Integer(i64::try_from(value)?))
}
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if ![6, 7].contains(&version) {
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
                    && genesis.version == 1
                    && genesis.tenant_id == tenant
                    && genesis.control_namespace_uuid == namespace
                    && genesis.previous_seq == stored.seq - 1
                    && genesis.previous_hash == stored.prev_hash,
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
    let mut expected: BTreeMap<&str, Vec<SqlRow>> = BTreeMap::new();
    expected.insert(
        "workload_meta",
        vec![vec![
            rusqlite::types::Value::Integer(1),
            rusqlite::types::Value::Integer(genesis_seq),
            sql_text(genesis_hash),
        ]],
    );
    expected.insert(
        "workload_bindings",
        projection
            .bindings
            .values()
            .map(|(binding, seq)| {
                Ok(vec![
                    sql_text(&binding.credential_id),
                    sql_int(binding.revision)?,
                    sql_text(&binding.sha256_digest),
                    sql_text(binding.subject.key()?),
                    sql_text(serde_json::to_string(binding)?),
                    rusqlite::types::Value::Integer(*seq),
                ])
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    expected.insert(
        "workload_binding_history",
        projection
            .history
            .values()
            .map(|(binding, seq)| {
                Ok(vec![
                    sql_text(&binding.credential_id),
                    sql_int(binding.revision)?,
                    sql_text(serde_json::to_string(binding)?),
                    rusqlite::types::Value::Integer(*seq),
                ])
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    expected.insert(
        "workload_digests",
        projection
            .digests
            .iter()
            .map(|(digest, id)| vec![sql_text(digest), sql_text(id)])
            .collect(),
    );
    expected.insert(
        "workload_operations",
        projection
            .operations
            .iter()
            .map(|(op, (hash, seq, id, revision))| {
                Ok(vec![
                    sql_text(op),
                    sql_text(hash),
                    rusqlite::types::Value::Integer(*seq),
                    sql_text(id),
                    sql_int(*revision)?,
                ])
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    expected.insert(
        "workload_worker_endpoints",
        projection
            .endpoints
            .values()
            .map(|(endpoint, seq)| {
                Ok(vec![
                    sql_text(&endpoint.worker_id),
                    sql_text(&endpoint.attempt_id),
                    sql_int(endpoint.generation)?,
                    rusqlite::types::Value::Integer(*seq),
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
                    .map(|col| row.get::<_, rusqlite::types::Value>(col))
                    .collect::<rusqlite::Result<SqlRow>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        anyhow::ensure!(
            expected.remove(table) == Some(actual),
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
