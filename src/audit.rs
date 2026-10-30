use crate::{
    model::*,
    policy,
    redaction::Redactor,
    tenancy::{LOCAL_TENANT, validate_tenant_id},
};
use anyhow::Context;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};

mod control;
mod storage;
use storage::StoreStorage;

#[derive(Debug, Serialize)]
pub struct Integrity {
    pub valid: bool,
    pub event_count: u64,
    pub head_hash: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFailure {
    Conflict,
    AmbiguousHistory,
    ApprovalRequired,
    InvalidRequest,
}
impl std::fmt::Display for DecisionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Conflict => "request identity already belongs to different content",
            Self::AmbiguousHistory => "decision history has missing or ambiguous metadata",
            Self::ApprovalRequired => "approved phase requires a durable review and its exact hash",
            Self::InvalidRequest => "invalid request identity or approval phase",
        })
    }
}
impl std::error::Error for DecisionFailure {}
pub struct Store {
    tenant_id: String,
    db: Mutex<Connection>,
    backing: StoreStorage,
    redactor: Redactor,
    max_events: u64,
    unhealthy: AtomicBool,
    validated_data_version: AtomicI64,
    supervision_lease: Arc<SupervisionOrigin>,
}

struct SupervisionOrigin {
    state: Mutex<SupervisionLease>,
    invalidated: AtomicBool,
}

// This capability is deliberately outside SQLite and is never serialized.
// Opening an existing database cannot recreate a fresh execution authority.
struct SupervisionLease {
    backing: (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity),
    namespace: Option<String>,
    initial_head: (i64, String),
    floor: (i64, String),
    fresh_origin: bool,
    active: bool,
    invalidated: bool,
}

struct CheckedTransaction<'a> {
    transaction: Transaction<'a>,
    lease: MutexGuard<'a, SupervisionLease>,
    activate_supervision: bool,
}
impl<'a> std::ops::Deref for CheckedTransaction<'a> {
    type Target = Transaction<'a>;
    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}
impl SupervisionLease {
    fn invalidate(&mut self) {
        self.active = false;
        self.invalidated = true;
    }
    fn verify(&self, backing: &StoreStorage, db: &Connection) -> anyhow::Result<()> {
        anyhow::ensure!(!self.invalidated, "supervision origin is invalidated");
        self.verify_prefix(backing, db)
    }
    // Diagnostic validation checks facts without restoring execution authority.
    fn verify_prefix(&self, backing: &StoreStorage, db: &Connection) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.backing == backing.identity_key(),
            "supervision backing identity diverged"
        );
        let actual: Option<String> = if self.floor.0 == 0 {
            Some("0".repeat(64))
        } else {
            db.query_row(
                "SELECT hash FROM events WHERE seq=?1",
                [self.floor.0],
                |r| r.get(0),
            )
            .optional()?
        };
        anyhow::ensure!(
            actual.as_ref() == Some(&self.floor.1),
            "audit rolled back below the live supervision witness"
        );
        if let Some(namespace) = &self.namespace {
            let actual: String = db.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            anyhow::ensure!(
                &actual == namespace,
                "supervision namespace diverged from live origin"
            );
        }
        Ok(())
    }
    fn untouched_fresh_origin(&self, db: &Connection) -> anyhow::Result<bool> {
        let head = audit_head(db)?;
        let workers: i64 = db.query_row("SELECT count(*) FROM workers", [], |r| r.get(0))?;
        Ok(self.fresh_origin
            && !self.invalidated
            && !self.active
            && head == self.initial_head
            && self.floor == self.initial_head
            && workers == 0)
    }
}
fn audit_head(db: &Connection) -> anyhow::Result<(i64, String)> {
    Ok(db
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, "0".repeat(64))))
}

const EVENTS_SCHEMA: &str = "CREATE TABLE events(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL,payload TEXT NOT NULL,kind TEXT NOT NULL,agent_id TEXT,sandbox_id TEXT,project_id TEXT,decision TEXT)";
const QUOTA_SCHEMA: &str =
    "CREATE TABLE quota(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL)";
const BINDING_SCHEMA: &str = "CREATE TABLE tenant_binding(singleton INTEGER PRIMARY KEY CHECK(singleton=1),tenant_id TEXT NOT NULL UNIQUE,schema_version INTEGER NOT NULL CHECK(schema_version=1))";
const SCHEMA: [(&str, &str, &str); 7] = [
    ("table", "events", EVENTS_SCHEMA),
    ("table", "quota", QUOTA_SCHEMA),
    ("table", "tenant_binding", BINDING_SCHEMA),
    (
        "index",
        "events_agent",
        "CREATE INDEX events_agent ON events(agent_id,seq)",
    ),
    (
        "index",
        "events_sandbox",
        "CREATE INDEX events_sandbox ON events(sandbox_id,seq)",
    ),
    (
        "index",
        "events_project",
        "CREATE INDEX events_project ON events(project_id,seq)",
    ),
    (
        "index",
        "events_kind",
        "CREATE INDEX events_kind ON events(kind,seq)",
    ),
];

// These are the internal indexes created by the exact PRIMARY KEY/UNIQUE
// constraints above and in control::SCHEMA. A reserved prefix alone is never
// evidence that a schema object is internal: writable_schema can create a
// trigger with such a name.
const AUTO_INDEX_COUNTS: [(&str, u32); 8] = [
    ("events", 1),
    ("tenant_binding", 1),
    ("control_meta", 2),
    ("workers", 4),
    ("worker_attempts", 6),
    ("worker_observations", 2),
    ("worker_allocations", 3),
    ("worker_mutations", 2),
];

fn normalized_schema(sql: &str) -> String {
    let normalized = sql
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    for kind in ["table", "index"] {
        if let Some(suffix) = normalized.strip_prefix(&format!("create {kind} if not exists ")) {
            return format!("create {kind} {suffix}");
        }
    }
    normalized
}

/// Recognize only the exact schema shipped by this application. Legacy version
/// zero may omit tenant_binding; a versioned database must never be repaired by
/// CREATE IF NOT EXISTS after a table or index has disappeared.
fn validate_schema(db: &Connection, allow_legacy: bool) -> anyhow::Result<bool> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        version == 1 || version == 2 || version == 3 || (allow_legacy && version == 0),
        "unsupported audit schema version"
    );
    let mut statement = db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema")?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if objects.is_empty() {
        anyhow::ensure!(
            allow_legacy && version == 0,
            "versioned audit schema is missing"
        );
        return Ok(true);
    }
    let automatic = AUTO_INDEX_COUNTS
        .iter()
        .chain(if version == 3 {
            control::supervision::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .filter(|(table, _)| {
            if *table == "events" {
                true
            } else if *table == "tenant_binding" {
                version != 0
                    || objects
                        .iter()
                        .any(|(kind, name, _, _)| kind == "table" && name == "tenant_binding")
            } else {
                version >= 2
            }
        })
        .flat_map(|(table, count)| {
            (1..=*count).map(move |ordinal| (format!("sqlite_autoindex_{table}_{ordinal}"), *table))
        })
        .collect::<Vec<_>>();
    for (kind, name, table, sql) in &objects {
        if sql.is_none() {
            anyhow::ensure!(
                kind == "index"
                    && automatic
                        .iter()
                        .any(|(expected_name, expected_table)| name == expected_name
                            && table == expected_table),
                "unexpected internal audit schema object"
            );
            continue;
        }
        let expected = SCHEMA
            .iter()
            .chain(if version >= 2 {
                control::SCHEMA.iter()
            } else {
                [].iter()
            })
            .chain(if version == 3 {
                control::supervision::SCHEMA.iter()
            } else {
                [].iter()
            })
            .find(|(expected_kind, expected_name, _)| {
                kind == expected_kind && name == expected_name
            });
        anyhow::ensure!(
            expected.is_some_and(|(_, _, expected_sql)| sql
                .as_ref()
                .is_some_and(|sql| normalized_schema(sql) == normalized_schema(expected_sql)))
                && table
                    == if kind == "table" {
                        name
                    } else if name == "workers_held" {
                        "workers"
                    } else if name == "helper_observations_helper" {
                        "helper_observations"
                    } else {
                        "events"
                    },
            "unexpected audit schema object"
        );
    }
    for &(kind, name, _) in SCHEMA
        .iter()
        .chain(if version >= 2 {
            control::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if version == 3 {
            control::supervision::SCHEMA.iter()
        } else {
            [].iter()
        })
    {
        if allow_legacy && version == 0 && name == "tenant_binding" {
            continue;
        }
        anyhow::ensure!(
            objects
                .iter()
                .any(|(actual_kind, actual_name, _, _)| actual_kind == kind && actual_name == name),
            "audit schema object is missing"
        );
    }
    for (name, table) in automatic {
        anyhow::ensure!(
            objects
                .iter()
                .any(|(kind, actual_name, actual_table, sql)| kind == "index"
                    && actual_name == &name
                    && actual_table == table
                    && sql.is_none()),
            "internal audit schema index is missing"
        );
    }
    Ok(false)
}

#[derive(Default, Debug, Clone, Deserialize)]
pub struct EventQuery {
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub limit: Option<usize>,
    pub q: Option<String>,
    pub agent_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub project_id: Option<String>,
    pub kind: Option<String>,
    pub decision: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub order: Option<String>,
    pub format: Option<String>,
}

fn digest(seq: i64, received: &str, prev: &str, payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"agentsandbox:audit:v1\0");
    h.update(seq.to_be_bytes());
    for s in [received, prev, payload] {
        h.update(s.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}
fn row_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let payload: String = row.get(4)?;
    let event = serde_json::from_str(&payload).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(StoredEvent {
        seq: row.get(0)?,
        received_at: row.get(1)?,
        prev_hash: row.get(2)?,
        hash: row.get(3)?,
        event,
    })
}

impl Store {
    pub fn open(path: &Path, redactor: Redactor, max_events: u64) -> anyhow::Result<Self> {
        Self::open_for_tenant(path, LOCAL_TENANT, redactor, max_events)
    }
    pub fn open_for_tenant(
        path: &Path,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
    ) -> anyhow::Result<Self> {
        validate_tenant_id(tenant_id)?;
        anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
        let (backing, _initialization_guard) = StoreStorage::open(path)?;
        let mut db = Connection::open_with_flags(
            backing.path(),
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        backing.verify(&db)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA trusted_schema=OFF;")?;
        // Unknown schemas are inspected without even changing journal mode.
        // Revalidate below while holding the write transaction to close races.
        validate_schema(&db, true)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;
            PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=16777216;")?;
        // A transaction serializes first-open binding across concurrent processes.
        // A preexisting audit can only be adopted by the historical local identity.
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        backing.verify(&tx)?;
        let empty = validate_schema(&tx, true)?;
        let initial_version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if empty {
            for (_, _, sql) in SCHEMA {
                tx.execute_batch(sql)?;
            }
            tx.execute("INSERT INTO quota(id,bytes) VALUES(1,0)", [])?;
        } else {
            // The only supported legacy migration adds the missing binding.
            // No events, quota rows, indexes or unknown schemas are repaired.
            tx.execute_batch(&BINDING_SCHEMA.replacen(
                "CREATE TABLE",
                "CREATE TABLE IF NOT EXISTS",
                1,
            ))?;
        }
        let binding: Option<(String, i64)> = tx
            .query_row(
                "SELECT tenant_id,schema_version FROM tenant_binding WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((bound, version)) = binding {
            anyhow::ensure!(
                bound == tenant_id && version == 1,
                "database tenant binding mismatch or unsupported schema"
            );
        } else {
            let existing: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM events)", [], |row| row.get(0))?;
            anyhow::ensure!(
                !existing || tenant_id == LOCAL_TENANT,
                "legacy audit requires explicit local identity; tenant migration is not automatic"
            );
            tx.execute(
                "INSERT INTO tenant_binding(singleton,tenant_id,schema_version) VALUES(1,?1,1)",
                [tenant_id],
            )?;
        }
        if empty {
            control::initialize(&tx, max_events, &redactor)?;
            tx.pragma_update(None, "user_version", 2)?;
        } else if initial_version == 0 {
            tx.pragma_update(None, "user_version", 1)?;
        }
        let initial_integrity = Self::check_integrity(&tx)?;
        let initial_valid = initial_integrity.valid
            && Self::validate_metadata(&tx, tenant_id).is_ok()
            && control::validate(&tx).is_ok();
        let validated_data_version =
            tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let initial_head = audit_head(&tx)?;
        let schema_version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let namespace = if schema_version >= 2 {
            Some(tx.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )?)
        } else {
            None
        };
        tx.commit()?;
        backing.verify(&db)?;
        let supervision_lease = Arc::new(SupervisionOrigin {
            state: Mutex::new(SupervisionLease {
                backing: backing.identity_key(),
                namespace,
                initial_head: initial_head.clone(),
                floor: initial_head,
                fresh_origin: backing.created_here() && empty && initial_valid,
                active: false,
                invalidated: !initial_valid,
            }),
            invalidated: AtomicBool::new(!initial_valid),
        });
        Ok(Self {
            tenant_id: tenant_id.into(),
            db: Mutex::new(db),
            backing,
            redactor,
            max_events,
            unhealthy: AtomicBool::new(!initial_valid),
            validated_data_version: AtomicI64::new(validated_data_version),
            supervision_lease,
        })
    }
    /// Another connection to this live origin; no caller-selected identity or path.
    /// An independent open remains maintenance-only even for this same pathname.
    pub fn sibling(&self) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.healthy(),
            "cannot share an unhealthy supervision origin"
        );
        self.backing.verify_names()?;
        let mut sibling = Self::open_for_tenant(
            self.backing.path(),
            &self.tenant_id,
            self.redactor.clone(),
            self.max_events,
        )?;
        anyhow::ensure!(
            sibling.backing.identity_key() == self.backing.identity_key(),
            "sibling backing differs from live origin"
        );
        sibling.supervision_lease = Arc::clone(&self.supervision_lease);
        let mut db = sibling
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = sibling.begin_checked(&mut db)?;
        sibling.commit_checked(tx)?;
        drop(db);
        self.backing.verify_names()?;
        Ok(sibling)
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn healthy(&self) -> bool {
        !self.unhealthy.load(Ordering::Acquire)
            && !self.supervision_lease.invalidated.load(Ordering::Acquire)
    }
    fn lock_supervision_lease(&self) -> anyhow::Result<MutexGuard<'_, SupervisionLease>> {
        self.supervision_lease.state.lock().map_err(|_| {
            self.unhealthy.store(true, Ordering::Release);
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
            anyhow::anyhow!("supervision lease lock poisoned")
        })
    }
    fn verify_storage(&self, db: &Connection) -> anyhow::Result<()> {
        let mut lease = self.lock_supervision_lease()?;
        let result = self
            .backing
            .verify(db)
            .and_then(|()| lease.verify(&self.backing, db));
        if result.is_err() {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
        }
        result
    }
    fn verify_diagnostic_backing(&self, db: &Connection) -> anyhow::Result<()> {
        let mut lease = self.lock_supervision_lease()?;
        let result = self.backing.verify(db);
        if result.is_err() {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
        }
        result
    }
    fn prepare_event(&self, mut event: Event) -> anyhow::Result<(Event, String)> {
        anyhow::ensure!(
            self.healthy(),
            "audit integrity failure is latched; recovery required"
        );
        event.validate()?;
        event.timestamp = chrono::DateTime::parse_from_rfc3339(&event.timestamp)?
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let event: Event =
            serde_json::from_value(self.redactor.value(&serde_json::to_value(event)?))?;
        let payload = serde_json::to_string(&event)?;
        anyhow::ensure!(
            payload.len() <= 131_072,
            "event exceeds 128 KiB after redaction"
        );
        Ok((event, payload))
    }
    pub fn append(&self, event: Event) -> anyhow::Result<StoredEvent> {
        anyhow::ensure!(
            !event.kind.starts_with("control."),
            "control events require typed mutation"
        );
        let (event, payload) = self.prepare_event(event)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        Ok(stored)
    }
    fn begin_checked<'a>(
        &'a self,
        db: &'a mut Connection,
    ) -> anyhow::Result<CheckedTransaction<'a>> {
        let mut lease = self.lock_supervision_lease()?;
        anyhow::ensure!(
            self.healthy() && !lease.invalidated,
            "audit integrity failure is latched; recovery required"
        );
        if let Err(error) = self.backing.verify(db) {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
            return Err(error);
        }
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Err(error) = self.backing.verify(&tx) {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
            return Err(error);
        }
        if let Err(error) = lease.verify(&self.backing, &tx) {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
            return Err(error);
        }
        // data_version changes for commits made by another SQLite connection,
        // including DDL. The write transaction keeps that validated snapshot
        // stable until this append is committed, without rescanning our own
        // unchanged chain for every event.
        let current_version: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
        if current_version != self.validated_data_version.load(Ordering::Acquire) {
            let verification = (|| -> anyhow::Result<()> {
                validate_schema(&tx, false)?;
                Self::validate_metadata(&tx, &self.tenant_id)?;
                anyhow::ensure!(Self::check_integrity(&tx)?.valid, "audit integrity failure");
                control::validate(&tx)?;
                Ok(())
            })();
            if let Err(error) = verification {
                self.unhealthy.store(true, Ordering::Release);
                lease.invalidate();
                self.supervision_lease
                    .invalidated
                    .store(true, Ordering::Release);
                return Err(error);
            }
            self.validated_data_version
                .store(current_version, Ordering::Release);
        }
        Ok(CheckedTransaction {
            transaction: tx,
            lease,
            activate_supervision: false,
        })
    }
    fn commit_checked(&self, tx: CheckedTransaction<'_>) -> anyhow::Result<()> {
        let CheckedTransaction {
            transaction,
            mut lease,
            activate_supervision,
        } = tx;
        let result = (|| {
            anyhow::ensure!(
                self.healthy(),
                "supervision origin invalidated before commit"
            );
            self.backing.verify(&transaction)?;
            lease.verify(&self.backing, &transaction)?;
            let next_head = audit_head(&transaction)?;
            anyhow::ensure!(
                next_head.0 >= lease.floor.0,
                "audit head regressed before commit"
            );
            let schema_version: i64 =
                transaction.pragma_query_value(None, "user_version", |r| r.get(0))?;
            let namespace = if schema_version >= 2 {
                Some(transaction.query_row(
                    "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?)
            } else {
                None
            };
            transaction.commit()?;
            self.backing.verify_names()?;
            // The shared guard still covers this publication, before an ACK or EmitOnce.
            lease.floor = next_head;
            lease.namespace = namespace;
            if activate_supervision {
                lease.active = true;
            }
            Ok(())
        })();
        if result.is_err() {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
        }
        result
    }
    fn append_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
    ) -> anyhow::Result<StoredEvent> {
        let existing = tx
            .query_row(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                [&event.id],
                row_event,
            )
            .optional()?;
        if let Some(old) = existing {
            anyhow::ensure!(
                serde_json::to_string(&old.event)? == payload,
                "event id conflict"
            );
            return Ok(old);
        }
        let (last, prev): (i64, String) = tx
            .query_row(
                "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, "0".repeat(64)));
        anyhow::ensure!(
            last < (self.max_events as i64),
            "audit capacity exhausted; archive before new executions"
        );
        let used: i64 = tx.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))?;
        anyhow::ensure!(
            used + payload.len() as i64 <= 256 * 1024 * 1024,
            "audit byte capacity exhausted"
        );
        let received = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seq = last + 1;
        let hash = digest(seq, &received, &prev, &payload);
        let decision = event.decision.as_ref().map(|d| match d.outcome {
            Outcome::Allow => "allow",
            Outcome::Deny => "deny",
            Outcome::Review => "review",
        });
        let mutation = (|| -> anyhow::Result<()> {
            let inserted = tx.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind,agent_id,sandbox_id,project_id,decision) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![seq,event.id,received,prev,hash,payload,event.kind,event.agent_id,event.sandbox_id,event.project_id,decision])?;
            let updated = tx.execute(
                "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
                [payload.len() as i64],
            )?;
            let persisted: Option<(i64, String)> = tx.query_row("SELECT seq,payload FROM events WHERE id=?1 AND received_at=?2 AND prev_hash=?3 AND hash=?4", params![event.id,received,prev,hash], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            let quota_after: i64 =
                tx.query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get(0))?;
            anyhow::ensure!(
                inserted == 1
                    && updated == 1
                    && persisted == Some((seq, payload.clone()))
                    && quota_after == used + payload.len() as i64,
                "audit append did not persist the expected event and quota"
            );
            Ok(())
        })();
        if let Err(error) = mutation {
            self.unhealthy.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(StoredEvent {
            seq,
            received_at: received,
            prev_hash: prev,
            hash,
            event,
        })
    }

    /// Lookup and optional insertion are serialized with the audit itself.
    /// Call with no proposal before external policy work, then recheck with a
    /// proposal afterwards. No database lock is held across that external work.
    pub fn decision(
        &self,
        request: &DecisionRequest,
        proposed: Option<(Decision, Value)>,
    ) -> anyhow::Result<Option<StoredEvent>> {
        if uuid::Uuid::parse_str(&request.request_id).is_err()
            || (!request.human_approved && request.approval_hash.is_some())
            || !policy::valid_provider_context(request)
        {
            return Err(DecisionFailure::InvalidRequest.into());
        }
        let hash = policy::request_hash(request);
        // Check the approval before even considering a cached allow.
        if request.human_approved && request.approval_hash.as_deref() != Some(hash.as_str()) {
            return Err(DecisionFailure::ApprovalRequired.into());
        }
        // Never persist or replay a key that masking would make unfindable.
        // Keep masking enabled, and reject this request instead of weakening
        // secret protection or manufacturing a second durable identity.
        let mut identity = json!({"kind":"security.decision","source":"gateway","provenance":"observed","request_id":request.request_id,"request_hash":hash,"profile":request.profile,"human_approved":request.human_approved});
        if let Some(mode) = request.execution_mode {
            // Absence retains the exact historical batch identity and payload.
            identity["execution_mode"] = serde_json::to_value(mode)?;
        }
        if let Some(resolution) = &request.provider_resolution {
            identity["provider_resolution"] = serde_json::to_value(resolution)?;
        }
        if self.redactor.value(&identity) != identity {
            return Err(DecisionFailure::InvalidRequest.into());
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(existing) = Self::lookup_decision(&tx, request, &hash)? {
            self.commit_checked(tx)?;
            return Ok(Some(existing));
        }
        let Some((decision, jev)) = proposed else {
            self.commit_checked(tx)?;
            return Ok(None);
        };
        let mut event = Event::new("security.decision");
        event.project_id = request.project_id.clone();
        event.sandbox_id = request.sandbox_id.clone();
        event.agent_id = request.agent_id.clone();
        event.session_id = request.session_id.clone();
        event.correlation_id = request.correlation_id.clone();
        event.trace_id = request.trace_id.clone();
        event.command = Some(request.command.clone());
        event.decision = Some(decision);
        event.data = json!({"request_id":request.request_id,"request_hash":hash,"profile":request.profile,"human_approved":request.human_approved,"jev":jev});
        if let Some(mode) = request.execution_mode {
            event.data["execution_mode"] = serde_json::to_value(mode)?;
        }
        if let Some(resolution) = &request.provider_resolution {
            event.data["provider_resolution"] = serde_json::to_value(resolution)?;
        }
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        Ok(Some(stored))
    }
    /// Missing is the historical batch form. A present unknown or null mode is
    /// ambiguous history, never permission to reinterpret it as batch.
    fn decision_execution_mode(data: &Value) -> anyhow::Result<Option<ExecutionModeV1>> {
        match data.get("execution_mode") {
            None => Ok(None),
            Some(value) => serde_json::from_value::<ExecutionModeV1>(value.clone())
                .map(Some)
                .map_err(|_| DecisionFailure::AmbiguousHistory.into()),
        }
    }

    fn decision_provider_resolution(
        data: &Value,
    ) -> anyhow::Result<Option<crate::provider::ProviderResolutionV1>> {
        match data.get("provider_resolution") {
            None => Ok(None),
            Some(value) => {
                let resolution: crate::provider::ProviderResolutionV1 =
                    serde_json::from_value(value.clone())
                        .map_err(|_| DecisionFailure::AmbiguousHistory)?;
                resolution
                    .validate()
                    .map_err(|_| DecisionFailure::AmbiguousHistory)?;
                Ok(Some(resolution))
            }
        }
    }

    fn lookup_decision(
        db: &Connection,
        request: &DecisionRequest,
        hash: &str,
    ) -> anyhow::Result<Option<StoredEvent>> {
        let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind='security.decision' AND json_extract(payload,'$.data.request_id')=?1 ORDER BY seq")?;
        let events = statement
            .query_map([&request.request_id], row_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut initial: Option<StoredEvent> = None;
        let mut approved: Option<StoredEvent> = None;
        for stored in events {
            let event = &stored.event;
            let data = &event.data;
            let Some(old_hash) = data.get("request_hash").and_then(Value::as_str) else {
                return Err(DecisionFailure::AmbiguousHistory.into());
            };
            if old_hash != hash {
                return Err(DecisionFailure::Conflict.into());
            }
            let Some(phase) = data.get("human_approved").and_then(Value::as_bool) else {
                return Err(DecisionFailure::AmbiguousHistory.into());
            };
            if event.source != "gateway"
                || event.provenance != "observed"
                || event.decision.is_none()
                || data.get("profile").and_then(Value::as_str) != Some(request.profile.as_str())
                || data.get("jev").is_none()
                || Self::decision_execution_mode(data)? != request.execution_mode
                || Self::decision_provider_resolution(data)? != request.provider_resolution
            {
                return Err(DecisionFailure::AmbiguousHistory.into());
            }
            let slot = if phase { &mut approved } else { &mut initial };
            if slot.replace(stored).is_some() {
                return Err(DecisionFailure::AmbiguousHistory.into());
            }
        }
        if approved.is_some()
            && !initial.as_ref().is_some_and(|event| {
                event
                    .event
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.outcome == Outcome::Review)
            })
        {
            return Err(DecisionFailure::AmbiguousHistory.into());
        }
        if let (Some(initial), Some(approved)) = (&initial, &approved)
            && approved.seq <= initial.seq
        {
            return Err(DecisionFailure::AmbiguousHistory.into());
        }
        if request.human_approved {
            if !initial.as_ref().is_some_and(|event| {
                event
                    .event
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.outcome == Outcome::Review)
            }) {
                return Err(DecisionFailure::ApprovalRequired.into());
            }
            Ok(approved)
        } else {
            Ok(initial)
        }
    }
    pub fn integrity(&self) -> anyhow::Result<Integrity> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let mut lease = self.lock_supervision_lease()?;
        let result = (|| -> anyhow::Result<Integrity> {
            self.backing.verify(&db)?;
            // Keep schema, quota and event checks on one consistent snapshot,
            // even when another legitimate Store is appending concurrently.
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
            validate_schema(&tx, false)?;
            Self::validate_metadata(&tx, &self.tenant_id)?;
            let mut integrity = Self::check_integrity(&tx)?;
            if integrity.valid {
                control::validate(&tx)?;
                if let Err(error) = lease.verify_prefix(&self.backing, &tx) {
                    integrity.valid = false;
                    integrity.error = Some(error.to_string());
                }
            }
            self.backing.verify(&tx)?;
            if integrity.valid {
                let head = audit_head(&tx)?;
                anyhow::ensure!(
                    head.0 >= lease.floor.0,
                    "audit head regressed during integrity check"
                );
                lease.floor = head;
            }
            Ok(integrity)
        })();
        if result.as_ref().map_or(true, |r| !r.valid) {
            self.unhealthy.store(true, Ordering::Release);
            lease.invalidate();
            self.supervision_lease
                .invalidated
                .store(true, Ordering::Release);
        }
        result
    }
    fn validate_metadata(db: &Connection, tenant_id: &str) -> anyhow::Result<()> {
        let binding: (i64, String, i64) = db.query_row(
            "SELECT singleton,tenant_id,schema_version FROM tenant_binding",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        anyhow::ensure!(
            binding == (1, tenant_id.to_owned(), 1),
            "database tenant binding mismatch"
        );
        let (rows, used): (i64, i64) = db.query_row(
            "SELECT count(*),COALESCE(SUM(bytes),0) FROM quota WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let actual: i64 = db.query_row(
            "SELECT COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events",
            [],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            rows == 1 && used == actual && used >= 0,
            "audit byte quota is inconsistent"
        );
        Ok(())
    }
    fn check_integrity(db: &Connection) -> anyhow::Result<Integrity> {
        let mut stmt=db.prepare("SELECT seq,received_at,prev_hash,hash,payload,id,kind,agent_id,sandbox_id,project_id,decision FROM events ORDER BY seq")?;
        let mut rows = stmt.query([])?;
        let mut prev = "0".repeat(64);
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let received: String = row.get(1)?;
            let previous: String = row.get(2)?;
            let hash: String = row.get(3)?;
            let payload: String = row.get(4)?;
            let parsed = serde_json::from_str::<Event>(&payload);
            let projection_valid = if let Ok(e) = parsed {
                let decision = e.decision.map(|d| {
                    match d.outcome {
                        Outcome::Allow => "allow",
                        Outcome::Deny => "deny",
                        Outcome::Review => "review",
                    }
                    .to_string()
                });
                e.id == row.get::<_, String>(5)?
                    && e.kind == row.get::<_, String>(6)?
                    && e.agent_id == row.get(7)?
                    && e.sandbox_id == row.get(8)?
                    && e.project_id == row.get(9)?
                    && decision == row.get(10)?
            } else {
                false
            };
            if seq != count + 1
                || previous != prev
                || hash != digest(seq, &received, &previous, &payload)
                || !projection_valid
            {
                return Ok(Integrity {
                    valid: false,
                    event_count: count as u64,
                    head_hash: prev,
                    error: Some(format!("integrity violation at sequence {seq}")),
                });
            }
            prev = hash;
            count += 1;
        }
        Ok(Integrity {
            valid: true,
            event_count: count as u64,
            head_hash: prev,
            error: None,
        })
    }
    pub fn head(&self) -> anyhow::Result<i64> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_diagnostic_backing(&db)?;
        Ok(db.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))?)
    }
    pub fn events(&self, q: &EventQuery) -> anyhow::Result<Vec<StoredEvent>> {
        let mut sql =
            String::from("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = vec![];
        for (col, value) in [
            ("agent_id", &q.agent_id),
            ("sandbox_id", &q.sandbox_id),
            ("project_id", &q.project_id),
            ("kind", &q.kind),
            ("decision", &q.decision),
        ] {
            if let Some(v) = value {
                sql.push_str(&format!(" AND {col}=?"));
                args.push(v.clone().into());
            }
        }
        if let Some(seq) = q.before {
            sql.push_str(" AND seq < ?");
            args.push(seq.into());
        }
        if let Some(seq) = q.after {
            sql.push_str(" AND seq > ?");
            args.push(seq.into());
        }
        if let Some(s) = &q.from {
            chrono::DateTime::parse_from_rfc3339(s).context("invalid from date")?;
            sql.push_str(" AND julianday(received_at)>=julianday(?)");
            args.push(s.clone().into());
        }
        if let Some(s) = &q.to {
            chrono::DateTime::parse_from_rfc3339(s).context("invalid to date")?;
            sql.push_str(" AND julianday(received_at)<=julianday(?)");
            args.push(s.clone().into());
        }
        if let Some(s) = &q.q {
            anyhow::ensure!(s.len() <= 256, "search too long");
            sql.push_str(" AND instr(lower(payload), lower(?))>0");
            args.push(s.clone().into());
        }
        sql.push_str(if q.order.as_deref() == Some("asc") {
            " ORDER BY seq ASC LIMIT ?"
        } else {
            " ORDER BY seq DESC LIMIT ?"
        });
        args.push((q.limit.unwrap_or(100).clamp(1, 1000) as i64).into());
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_storage(&db)?;
        let mut stmt = db.prepare(&sql)?;
        Ok(stmt
            .query_map(rusqlite::params_from_iter(args), row_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn entities(&self, sandboxes: bool) -> anyhow::Result<Vec<Value>> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_storage(&db)?;
        let mut stmt = db.prepare(if sandboxes {
            "SELECT payload FROM events WHERE sandbox_id IS NOT NULL ORDER BY seq"
        } else {
            "SELECT payload FROM events WHERE agent_id IS NOT NULL ORDER BY seq"
        })?;
        let mut out: BTreeMap<String, Value> = BTreeMap::new();
        let mut declared = crate::declared_agents::DeclaredAgentProjection::default();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let e: Event = serde_json::from_str(&row.get::<_, String>(0)?)?;
            if !sandboxes {
                declared.observe(&e);
                if e.kind == "agent.declared"
                    && e.provenance == "declared"
                    && (e.data.get("provider_agent_id").is_some()
                        || e.data.get("declared_agent_id").is_some())
                {
                    continue;
                }
            }
            let id = if sandboxes {
                e.sandbox_id.clone()
            } else {
                e.agent_id.clone()
            };
            let Some(id) = id else { continue };
            let item=out.entry(id.clone()).or_insert_with(||if sandboxes {json!({"id":id,"engine":null,"image":null,"profile":null,"status":"unknown","project_id":e.project_id,"session_id":e.session_id,"network":null,"started_at":null,"resources":null,"mounts":[]})} else {json!({"id":id,"provider":null,"name":id,"status":"unknown","parent_id":e.parent_agent_id,"project_id":e.project_id,"sandbox_id":e.sandbox_id,"model":null,"session_id":e.session_id,"started_at":null,"last_activity":null,"task":null,"compatibility":e.coverage,"provenance":e.provenance})});
            if e.provenance == "observed" {
                let state = match e.kind.as_str() {
                    "agent.started" | "command.started" => Some("running"),
                    "agent.completed" | "agent.stopped" | "command.completed" => Some("completed"),
                    "agent.failed" | "command.failed" | "sandbox.failed" => Some("error"),
                    "runtime.execution.error" => Some("unknown"),
                    "sandbox.created" | "session.started" => Some("ready"),
                    "sandbox.stopped" => Some("stopped"),
                    "security.decision"
                        if e.decision
                            .as_ref()
                            .is_some_and(|d| d.outcome == Outcome::Deny) =>
                    {
                        Some("blocked")
                    }
                    _ => None,
                };
                if let Some(status) = state {
                    item["status"] = json!(status);
                }
                if e.kind == "command.started"
                    || e.kind == "agent.started"
                    || e.kind == "sandbox.created"
                {
                    item["started_at"] = json!(e.timestamp);
                }
                if e.kind == "sandbox.metrics" {
                    item["resources"] = e.data.clone();
                }
            }
            if !sandboxes {
                item["last_activity"] = json!(e.timestamp);
            }
            let fields: &[&str] = if sandboxes {
                &["engine", "image", "profile", "network", "mounts"]
            } else {
                &["provider", "name", "model", "task"]
            };
            if item.get("field_provenance").is_none() {
                item["field_provenance"] = json!({});
            }
            if e.provenance == "observed" && !sandboxes {
                item["compatibility"] = json!(e.coverage);
            }
            for key in fields {
                if let Some(v) = e.data.get(*key).filter(|v| !v.is_null()) {
                    let prior_observed = item["field_provenance"][*key] == "observed";
                    if (sandboxes && e.provenance != "observed")
                        || (prior_observed && e.provenance != "observed")
                    {
                        continue;
                    }
                    item[*key] = v.clone();
                    item["field_provenance"][*key] = json!(e.provenance);
                }
            }
        }
        let mut items: Vec<Value> = out.into_values().collect();
        if !sandboxes {
            items.extend(declared.finish());
        }
        Ok(items)
    }
    pub fn overview(&self) -> anyhow::Result<Value> {
        let agents = self.entities(false)?;
        let sandboxes = self.entities(true)?;
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_storage(&db)?;
        let count = |condition: &str| -> anyhow::Result<i64> {
            Ok(db.query_row(
                &format!("SELECT count(*) FROM events WHERE {condition}"),
                [],
                |r| r.get(0),
            )?)
        };
        let pending:i64=db.query_row("SELECT count(*) FROM events e WHERE e.kind='security.decision' AND e.decision='review' AND NOT EXISTS(SELECT 1 FROM events later WHERE later.kind='security.decision' AND later.seq>e.seq AND json_extract(later.payload,'$.data.request_id')=json_extract(e.payload,'$.data.request_id'))",[],|r|r.get(0))?;
        Ok(
            json!({"counts":{"agents":agents.iter().filter(|a|a["status"]=="running" && a["provenance"]=="observed").count(),"subagents":agents.iter().filter(|a|a["status"]=="running" && a["provenance"]=="observed" && !a["parent_id"].is_null()).count(),"declared_agents":agents.iter().filter(|a|a["status"]=="running" && a["provenance"]=="declared").count(),"declared_subagents":agents.iter().filter(|a|a["status"]=="running" && a["provenance"]=="declared" && !a["parent_id"].is_null()).count(),"sandboxes":sandboxes.len(),"commands":count("kind='command.requested'")?,"allowed":count("kind='security.decision' AND decision='allow'")?,"denied":count("kind='security.decision' AND decision='deny'")?,"pending":pending,"security":count("kind LIKE 'security.%'")?},"health":{"collector":if self.healthy(){"ok"}else{"degraded"},"engine":"unknown","jev":"disabled"},"resources":{"cpu_percent":null,"memory_bytes":null},"coverage":"observed","timestamp":chrono::Utc::now().to_rfc3339(),"sequence":db.query_row("SELECT COALESCE(MAX(seq),0) FROM events",[],|r|r.get::<_,i64>(0))?}),
        )
    }
}
