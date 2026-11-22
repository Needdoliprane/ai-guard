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
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
};

pub mod authentication;
mod capacity;
pub(crate) mod capacity_policy;
pub use capacity::WorkloadAuditPolicyV1;
pub use capacity_policy::{
    AuditCapacityFailureV1, AuditCapacityPolicyV1, AuditCapacityV1, DEFAULT_PHYSICAL_BYTES,
    MAX_SQLITE_PHYSICAL_BYTES,
};
pub(crate) mod decision_codec;
pub use decision_codec::DecisionFailure;
pub(crate) mod entities;
pub(crate) mod event_codec;
mod event_query;
pub use event_query::EventQuery;
pub(crate) use event_query::InvalidEventFilter;
pub(crate) mod overview;
pub(crate) mod supervision_admission;
pub(crate) mod supervision_genesis;
pub(crate) mod worker_admission;
pub(crate) mod worker_transition;
use event_codec::digest;

pub(crate) const MAX_EVENT_BATCH: usize = 64;

pub(crate) const D5_CONTROL_EVENT_KINDS: &[&str] = &[
    "broker.abandon-before-claim.requested",
    "broker.retirement.pending",
    "broker.retirement.final",
    RETIREMENT_RESUME_EVENT_KIND,
];

pub(crate) const RETIREMENT_RESUME_EVENT_KIND: &str = "broker.retirement.resume.requested";

/// Closed request data only. This is neither authentication nor a permission;
/// the broker must retain the actual original and confirm its exact audit ACK.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetirementResumeRecordV1 {
    pub(crate) version: u32,
    pub(crate) operation_id: String,
    pub(crate) command_id: String,
    pub(crate) retirement_id: String,
    pub(crate) arm_id: String,
    pub(crate) original_epoch: String,
    pub(crate) generation: u64,
    pub(crate) identity_sha256: String,
    pub(crate) evidence_sha256: String,
    pub(crate) pending_ack_sha256: String,
    pub(crate) deadline_ns_decimal: String,
}

pub(crate) fn retirement_resume_record(event: &Event) -> anyhow::Result<RetirementResumeRecordV1> {
    anyhow::ensure!(
        event.kind == RETIREMENT_RESUME_EVENT_KIND
            && event.source == "ai-guard-broker"
            && event.provenance == "observed"
            && event.coverage == "native"
            && event.parent_agent_id.is_none()
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none(),
        "invalid retirement resume attribution"
    );
    let object = event
        .data
        .as_object()
        .context("invalid retirement resume shape")?;
    anyhow::ensure!(
        object.len() == 11
            && object.iter().all(|(key, value)| key.len() <= 32
                && match value {
                    Value::String(value) => value.len() <= 64,
                    Value::Number(value) => value.is_u64(),
                    _ => false,
                }),
        "retirement resume shape exceeds its bound"
    );
    let body: RetirementResumeRecordV1 =
        serde_json::from_value(event.data.clone()).context("invalid retirement resume request")?;
    let canonical_uuid = |value: &str| {
        uuid::Uuid::parse_str(value)
            .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
    };
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    anyhow::ensure!(
        body.version == 1
            && body.generation != u64::MAX
            && body.command_id == event.id
            && [
                &body.operation_id,
                &body.command_id,
                &body.retirement_id,
                &body.arm_id,
                &body.original_epoch
            ]
            .iter()
            .all(|value| canonical_uuid(value))
            && [
                &body.identity_sha256,
                &body.evidence_sha256,
                &body.pending_ack_sha256
            ]
            .iter()
            .all(|value| digest(value))
            && body
                .deadline_ns_decimal
                .parse::<u128>()
                .is_ok_and(|n| n > 0 && n.to_string() == body.deadline_ns_decimal),
        "noncanonical retirement resume request"
    );
    Ok(body)
}

/// Closed delivery contract for the authenticated runner audit route. A kind
/// alone never selects a capacity class; the trusted append method does so.
pub(crate) fn control_delivery_kind(kind: &str) -> bool {
    D5_CONTROL_EVENT_KINDS.contains(&kind)
        || [
            "command.requested",
            "command.started",
            "command.completed",
            "command.failed",
            "runtime.submitted",
            "runtime.invocation.bound",
            "runtime.terminal.attach.authorized",
            "runtime.cleanup.failed",
            "workspace.import.requested",
            "workspace.import.created",
            "workspace.import.started",
            "workspace.import.completed",
            "workspace.import.recovered",
            "workspace.import.cleanup.failed",
            "workspace.scan.requested",
            "workspace.scan.created",
            "workspace.scan.started",
            "workspace.scan.completed",
            "workspace.export.requested",
            "workspace.export.completed",
            "workspace.diff.completed",
            "observer.coverage",
        ]
        .contains(&kind)
}
mod broker_checkpoint_migration;
#[cfg(test)]
mod broker_checkpoint_migration_tests;
mod broker_registry;
#[cfg(feature = "mysql-storage")]
pub(crate) use broker_registry::prepared_admin as prepared_broker_admin;
#[cfg(test)]
mod capacity_tests;
#[cfg(test)]
mod collision_isolation_tests;
mod control;
pub(crate) mod egress;
pub mod egress_decisions;
mod human;
mod human_brokers;
mod human_modern;
pub(crate) use human_modern::{HumanDiscoveryGeneration, HumanDiscoveryStale};
mod human_configuration;
mod human_grants;
mod human_memberships;
pub(crate) use human_configuration::HumanConfigurationStoreFactsV1;
pub(crate) use human_grants::HumanGrantFrameV1;
pub(crate) use human_memberships::{HumanMembershipFrameV1, HumanMembershipStorePageV1};
mod human_rights;
mod human_summary;
pub(crate) use human_summary::HumanSummaryFrameV1;
mod human_usage;
pub(crate) use human_usage::HumanUsageFrameV1;
mod human_workers;
pub(crate) mod identity;
pub(crate) mod identity_markers;
mod kubernetes_admission;
pub(crate) mod kubernetes_session;
mod metadata;
mod review_access;
mod review_authority;
mod review_authority_migration;
mod review_binding;
mod review_format14;
mod review_startup;
pub(crate) use kubernetes_admission::{
    ConfirmedKubernetesCreateResult, FreshKubernetesCreateClaimCommit,
    KubernetesCreateAuthorization, PreparedRecovery,
};
pub(crate) use metadata::MetadataLookup;
pub(crate) use review_access::{HumanSourceAccess, StoreOperationClass};
pub(crate) use review_authority::AuthorityReceiptV1;
pub(crate) use review_authority_migration::{
    ReviewSourceMigrationState, VerifiedReviewSourceBackup,
};
pub(crate) use review_startup::{
    ReviewStoreMaintenance, ReviewStoreMaintenanceWriter, ReviewStoreStartup,
};
#[cfg(test)]
pub(crate) mod kubernetes_admission_state_tests;
#[cfg(test)]
mod kubernetes_admission_tests;
#[cfg(test)]
mod kubernetes_policy_tests;
#[cfg(test)]
mod kubernetes_session_migration_tests;
#[cfg(test)]
pub(crate) mod read_test_observer;
mod relay;
mod relay_admission;
pub(crate) mod workload;
pub(crate) use broker_registry::owned_supervision::{OwnedBrokerWindow, OwnedRetainedSourceFacts};
pub(crate) use human::HumanProfileSnapshotV1;
pub(crate) use human::{HumanEventFilterV1, HumanEventPollV1, HumanStorePageV1};
pub(crate) use human_brokers::HumanBrokerFrameV1;
pub(crate) use human_brokers::owned::{OwnedHumanBrokerPageV1, OwnedHumanBrokerStorePageV1};
pub(crate) use human_workers::HumanWorkerFrameV1;
pub(crate) use worker_transition::expected_worker_transition;
pub(crate) use workload::owned_broker_identity::OwnedBrokerBindingStepV1;
pub mod retention;
pub mod signed_checkpoint;
mod storage;
use storage::{InitializationGuard, StoreStorage};
#[cfg(test)]
mod resource_tests;

/// A stored event differs from an input event and the batch was rolled back.
#[derive(Debug)]
pub struct BatchConflict;

impl std::fmt::Display for BatchConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event batch conflicts with a stored event")
    }
}

impl std::error::Error for BatchConflict {}

#[derive(Debug)]
struct EventIdConflict;

impl std::fmt::Display for EventIdConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event id conflict")
    }
}

impl std::error::Error for EventIdConflict {}

#[derive(Debug, Serialize)]
pub struct Integrity {
    pub valid: bool,
    pub event_count: u64,
    pub head_hash: String,
    pub error: Option<String>,
}

pub struct Store {
    tenant_id: String,
    db: Mutex<Connection>,
    backing: StoreStorage,
    redactor: Redactor,
    max_events: u64,
    /// Payload byte cap of this store (`capacity_policy::PHYSICAL_BYTES` by default).
    physical_bytes: u64,
    capacity_policy: AuditCapacityPolicyV1,
    workload_audit_policy: WorkloadAuditPolicyV1,
    workload_audit_policy_explicit: bool,
    relay_retention_policy: crate::relay::RelayRetentionPolicyV1,
    egress_policy: Option<crate::control::egress_decision::EgressPolicySnapshot>,
    unhealthy: AtomicBool,
    validated_data_version: AtomicI64,
    validated_total_changes: AtomicU64,
    supervision_lease: Arc<SupervisionOrigin>,
    // Only a verified modern opener may retain this capability. Legacy
    // constructors leave it absent; an epoch or policy DTO cannot install it.
    review_authority: Option<review_authority::RetainedReviewAuthority>,
    #[cfg(test)]
    read_test_observer: read_test_observer::ReadTestObserver,
}

struct SupervisionOrigin {
    state: Mutex<SupervisionLease>,
    capacity: Mutex<capacity::Cache>,
    positive_bindings: arc_swap::ArcSwap<metadata::PositiveBindings>,
    invalidated: AtomicBool,
    suspended: AtomicBool,
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
    pending_activation: bool,
    invalidated: bool,
}

/// Verified connection state shared by legacy construction and the private
/// review openers. This cache input never contains source/epoch authority.
struct PreparedStoreState {
    initial_valid: bool,
    fresh_origin: bool,
    validated_data_version: i64,
    initial_head: (i64, String),
    namespace: Option<String>,
    capacity: capacity::Cache,
    positive_bindings: metadata::PositiveBindings,
}

pub(crate) struct CheckedTransaction<'a> {
    transaction: Transaction<'a>,
    connection: &'a Connection,
    origin: &'a Store,
    lease: MutexGuard<'a, SupervisionLease>,
    source_access: Option<&'a HumanSourceAccess<'a>>,
    operation_class: StoreOperationClass,
    activate_supervision: bool,
    positive_bindings: Option<metadata::PositiveBindings>,
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
        self.pending_activation = false;
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
const WORKLOAD_INDEX_SCHEMA: [(&str, &str, &str); 1] = [(
    "index",
    "worker_mutations_worker_sequence",
    "CREATE INDEX worker_mutations_worker_sequence ON worker_mutations(worker_id,event_seq) WHERE action IN ('admit','prepare','create_issued','created','start_claim','observe','unknown','stop','cancel_reserved','cleanup_result','supervised_admit','armed_start_claim')",
)];
const WORKER_SNAPSHOT_PREDICATE: &str = "action IN ('admit','prepare','create_issued','created','start_claim','observe','unknown','stop','cancel_reserved','cleanup_result','supervised_admit','armed_start_claim')";

fn schema_objects(
    version: i64,
    retention: bool,
) -> impl Iterator<Item = &'static (&'static str, &'static str, &'static str)> {
    // Format14 inherits the complete, closed13 profile. Its extensions are
    // explicit; no future version can inherit it by a range comparison.
    let inherited = if matches!(version, 14 | 15) {
        13
    } else {
        version
    };
    inherited_schema_objects(inherited)
        .chain(review_format14::schema_objects().filter(move |_| matches!(version, 14 | 15)))
        .chain(egress::SCHEMA.iter().filter(move |_| version == 15))
        // The retention ledger exists exactly when its chain marker does.
        .chain(retention::ledger::SCHEMA.iter().filter(move |_| retention))
}

fn inherited_schema_objects(
    version: i64,
) -> impl Iterator<Item = &'static (&'static str, &'static str, &'static str)> {
    SCHEMA
        .iter()
        .chain(if version >= 2 {
            control::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(
            if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
                control::supervision::SCHEMA.iter()
            } else {
                [].iter()
            },
        )
        .chain(if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            identity::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            workload::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            broker_registry::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            relay::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            WORKLOAD_INDEX_SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [8, 9, 10, 11, 12, 13].contains(&version) {
            kubernetes_admission::SCHEMA.iter()
        } else {
            [].iter()
        })
        .chain(if [11, 12, 13].contains(&version) {
            kubernetes_session::SCHEMA.iter()
        } else {
            [].iter()
        })
}

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
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version)
            || (allow_legacy && version == 0),
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
    let retention = objects
        .iter()
        .any(|(kind, name, _, _)| kind == "table" && name == "events")
        && retention::ledger::capability_present(db)?;
    anyhow::ensure!(
        !retention || retention::ledger::format_admits(version),
        "audit retention requires schema13, 14 or 15"
    );
    let inherited = if matches!(version, 14 | 15) {
        13
    } else {
        version
    };
    // The Kubernetes session effect ledger is a capability family: its
    // tables exist exactly when its genesis marker does (no format change).
    let effect_capability: &[(&str, &str, &str)] =
        if [11, 12, 13, 14, 15].contains(&version) && kubernetes_session::effects::enabled(db)? {
            &kubernetes_session::effects::SCHEMA
        } else {
            &[]
        };
    let effect_indexes: &[(&str, u32)] = if effect_capability.is_empty() {
        &[]
    } else {
        &kubernetes_session::effects::AUTO_INDEX_COUNTS
    };
    let automatic = AUTO_INDEX_COUNTS
        .iter()
        .chain(
            if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&inherited) {
                control::supervision::AUTO_INDEX_COUNTS.iter()
            } else {
                [].iter()
            },
        )
        .chain(if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&inherited) {
            identity::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&inherited) {
            workload::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&inherited) {
            broker_registry::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(if [6, 7, 8, 9, 10, 11, 12, 13].contains(&inherited) {
            relay::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(if [8, 9, 10, 11, 12, 13].contains(&inherited) {
            kubernetes_admission::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(if [11, 12, 13].contains(&inherited) {
            kubernetes_session::AUTO_INDEX_COUNTS.iter()
        } else {
            [].iter()
        })
        .chain(review_format14::auto_index_counts().filter(move |_| matches!(version, 14 | 15)))
        .chain(
            egress::AUTO_INDEX_COUNTS
                .iter()
                .filter(move |_| version == 15),
        )
        .chain(
            retention::ledger::AUTO_INDEX_COUNTS
                .iter()
                .filter(move |_| retention),
        )
        .chain(effect_indexes.iter())
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
        let expected = schema_objects(version, retention)
            .chain(effect_capability.iter())
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
                    } else if name == "kubernetes_creates_held" {
                        "kubernetes_creates"
                    } else if name == "kubernetes_session_reservations_held" {
                        "kubernetes_session_reservations"
                    } else if name == "helper_observations_helper" {
                        "helper_observations"
                    } else if name == "identity_credentials_principal" {
                        "identity_credentials"
                    } else if name == "identity_memberships_principal" {
                        "identity_memberships"
                    } else if name == "identity_resources_owner" {
                        "identity_resources"
                    } else if name == "identity_shares_principal" {
                        "identity_shares"
                    } else if name == "worker_mutations_worker_sequence" {
                        "worker_mutations"
                    } else if name == "workload_binding_history_prefix" {
                        "workload_binding_history"
                    } else if name == "workload_bindings_subject" {
                        "workload_bindings"
                    } else if name == "broker_registry_operations_prefix" {
                        "broker_registry_operations"
                    } else if matches!(
                        name.as_str(),
                        "relay_messages_retained" | "relay_messages_rate" | "relay_messages_inbox"
                    ) {
                        "relay_messages"
                    } else if matches!(
                        name.as_str(),
                        "runner_review_bindings_active_credential"
                            | "runner_review_bindings_active_tenant"
                    ) {
                        "runner_review_bindings"
                    } else {
                        "events"
                    },
            "unexpected audit schema object"
        );
    }
    for &(kind, name, _) in schema_objects(version, retention).chain(effect_capability.iter()) {
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
    if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
        identity::validate_human_access_capability(db, version)?;
    }
    human_brokers::validate_capability(db, version)?;
    human_workers::validate_capability(db, version)?;
    human_rights::validate_capability(db, version)?;
    // Old profiles, including audit-only1, must never contain a future
    // reserved egress family. Exact15's complete replay runs under control.
    if version != 15 {
        egress::validate(db)?;
    }
    Ok(false)
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
        let (backing, initialization_guard) = StoreStorage::open(path)?;
        Self::open_prepared_for_tenant(
            backing,
            initialization_guard,
            tenant_id,
            redactor,
            max_events,
            true,
        )
    }
    /// Open maintenance data without creating a missing database, initializing
    /// an empty file or adopting an unversioned audit.
    pub fn open_existing_for_tenant(
        path: &Path,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
    ) -> anyhow::Result<Self> {
        validate_tenant_id(tenant_id)?;
        anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
        let (backing, guard) = StoreStorage::open_existing(path)?;
        Self::open_prepared_for_tenant(backing, guard, tenant_id, redactor, max_events, false)
    }
    /// Create a new audit origin and its exclusive pre-supervision backup.
    /// Existing databases and sidecars are refused before any SQLite open.
    /// Reopening the result does not recreate its live execution authority.
    pub fn create_supervised_for_tenant(
        path: &Path,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
        backup_path: &Path,
    ) -> anyhow::Result<Self> {
        validate_tenant_id(tenant_id)?;
        anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
        let (backing, initialization_guard) = StoreStorage::create_new(path)?;
        let target_parent = crate::control_fs::Directory::open_existing(
            backing
                .path()
                .parent()
                .context("new supervised audit parent is missing")?,
        )?;
        target_parent.durable_bootstrap()?;
        let store = Self::open_prepared_for_tenant(
            backing,
            initialization_guard,
            tenant_id,
            redactor,
            max_events,
            true,
        )?;
        // The prepared-open guard has now been released. The backup may use
        // the same parent and must acquire its own initialization guard.
        let backup_path = crate::control_fs::absolute_path(backup_path)?;
        let backup_parent = crate::control_fs::Directory::open_or_create(
            backup_path
                .parent()
                .context("new supervised backup parent is missing")?,
        )?;
        backup_parent.durable_bootstrap()?;
        store.migrate_control_v3(&backup_path)?;
        let state = store.supervision_state()?;
        anyhow::ensure!(
            state.schema_version == 3 && state.active,
            "new supervised audit origin could not establish live execution authority"
        );
        Ok(store)
    }
    fn open_prepared_for_tenant(
        backing: StoreStorage,
        _initialization_guard: InitializationGuard,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
        allow_initialization: bool,
    ) -> anyhow::Result<Self> {
        let mut flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        if !allow_initialization {
            flags.remove(OpenFlags::SQLITE_OPEN_CREATE);
        }
        let mut db = Connection::open_with_flags(backing.path(), flags)?;
        backing.verify(&db)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA trusted_schema=OFF;")?;
        // Descriptive format14 validation is also used by maintenance. A
        // legacy opener must not thereby acquire access or alter its journal.
        let prepared_format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if matches!(prepared_format, 14 | 15) {
            review_access::require_legacy(&db)?;
        }
        // Unknown schemas are inspected without even changing journal mode.
        // Revalidate below while holding the write transaction to close races.
        validate_schema(&db, allow_initialization)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;
            PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=16777216;")?;
        // A transaction serializes first-open binding across concurrent processes.
        // A preexisting audit can only be adopted by the historical local identity.
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        backing.verify(&tx)?;
        let initial_version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        // A maintenance writer can commit format14 after the initial probe.
        // Recheck inside this writer before any binding or projection access.
        // Format0 still follows the ordinary empty-database initialization.
        if matches!(initial_version, 14 | 15) {
            review_access::require_legacy(&tx)?;
        }
        let empty = validate_schema(&tx, allow_initialization)?;
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
            anyhow::ensure!(
                allow_initialization,
                "existing audit tenant binding is missing"
            );
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
        let mut capacity = capacity::Cache::default();
        let capacity_prefix = if initial_valid {
            Some(capacity.prepare(&tx)?)
        } else {
            None
        };
        let positive_bindings = if initial_valid {
            metadata::PositiveBindings::load(&tx)?
        } else {
            metadata::PositiveBindings::default()
        };
        tx.commit()?;
        backing.verify(&db)?;
        if let Some(prefix) = capacity_prefix {
            capacity.publish(prefix);
        }
        let fresh_origin = backing.created_here() && empty && initial_valid;
        Ok(Self::from_prepared_connection(
            backing,
            db,
            tenant_id,
            redactor,
            max_events,
            PreparedStoreState {
                initial_valid,
                fresh_origin,
                validated_data_version,
                initial_head,
                namespace,
                capacity,
                positive_bindings,
            },
        ))
    }
    fn from_prepared_connection(
        backing: StoreStorage,
        db: Connection,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
        state: PreparedStoreState,
    ) -> Self {
        let PreparedStoreState {
            initial_valid,
            fresh_origin,
            validated_data_version,
            initial_head,
            namespace,
            capacity,
            positive_bindings,
        } = state;
        let validated_total_changes = db.total_changes();
        let supervision_lease = Arc::new(SupervisionOrigin {
            capacity: Mutex::new(capacity),
            positive_bindings: arc_swap::ArcSwap::from_pointee(positive_bindings),
            state: Mutex::new(SupervisionLease {
                backing: backing.identity_key(),
                namespace,
                initial_head: initial_head.clone(),
                floor: initial_head,
                fresh_origin,
                active: false,
                pending_activation: false,
                invalidated: !initial_valid,
            }),
            invalidated: AtomicBool::new(!initial_valid),
            suspended: AtomicBool::new(false),
        });
        Self {
            tenant_id: tenant_id.into(),
            db: Mutex::new(db),
            backing,
            redactor,
            max_events,
            physical_bytes: capacity::PHYSICAL_BYTES,
            capacity_policy: AuditCapacityPolicyV1::resolved(max_events),
            workload_audit_policy: WorkloadAuditPolicyV1::resolved(
                max_events,
                &AuditCapacityPolicyV1::resolved(max_events),
            ),
            workload_audit_policy_explicit: false,
            relay_retention_policy: crate::relay::RelayRetentionPolicyV1::default(),
            egress_policy: None,
            unhealthy: AtomicBool::new(!initial_valid),
            validated_data_version: AtomicI64::new(validated_data_version),
            validated_total_changes: AtomicU64::new(validated_total_changes),
            supervision_lease,
            review_authority: None,
            #[cfg(test)]
            read_test_observer: read_test_observer::ReadTestObserver::default(),
        }
    }
    /// Another connection to this live origin; no caller-selected identity or path.
    /// An independent open remains maintenance-only even for this same pathname.
    pub fn sibling(&self) -> anyhow::Result<Self> {
        self.sibling_with_redactor(self.redactor.clone())
    }
    /// An immutable masking union attached to this same backing and live origin.
    /// The caller's view and the shared supervision capability remain unchanged.
    pub fn with_additional_redaction(
        self: &Arc<Self>,
        additional: &Redactor,
    ) -> anyhow::Result<Arc<Self>> {
        if self.redactor.covers(additional) {
            return Ok(Arc::clone(self));
        }
        Ok(Arc::new(
            self.sibling_with_redactor(self.redactor_union(additional))?,
        ))
    }
    pub(crate) fn redactor_union(&self, additional: &Redactor) -> Redactor {
        self.redactor.union(additional)
    }
    fn sibling_with_redactor(&self, redactor: Redactor) -> anyhow::Result<Self> {
        self.recover_before_access()?;
        anyhow::ensure!(
            self.healthy(),
            "cannot share an unhealthy supervision origin"
        );
        self.backing.verify_names()?;
        let mut sibling = Self::open_for_tenant(
            self.backing.path(),
            &self.tenant_id,
            redactor,
            self.max_events,
        )?;
        // The parent's resolved policy is immutable, including disabled defaults
        // for small stores. Do not resolve defaults again on this connection.
        sibling.physical_bytes = self.physical_bytes;
        sibling.capacity_policy = self.capacity_policy.clone();
        sibling.workload_audit_policy = self.workload_audit_policy.clone();
        sibling.workload_audit_policy_explicit = self.workload_audit_policy_explicit;
        sibling.relay_retention_policy = self.relay_retention_policy.clone();
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
        self.permanently_healthy() && !self.supervision_lease.suspended.load(Ordering::Acquire)
    }
    fn permanently_healthy(&self) -> bool {
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
    fn invalidate_storage(&self, lease: &mut SupervisionLease) {
        self.unhealthy.store(true, Ordering::Release);
        lease.invalidate();
        self.supervision_lease
            .invalidated
            .store(true, Ordering::Release);
    }
    fn storage_failure(&self, lease: &mut SupervisionLease, error: &anyhow::Error) {
        if review_access::admission_refusal(error, false) {
            return;
        }
        if storage::descriptor_shortage(error) || metadata::preparation_unavailable(error) {
            // Do not invalidate or recreate the live execution capability. It
            // remains closed until its exact backing and witness are validated.
            self.supervision_lease
                .suspended
                .store(true, Ordering::Release);
        } else {
            self.invalidate_storage(lease);
        }
    }
    fn recover_before_access(&self) -> anyhow::Result<()> {
        if !self.supervision_lease.suspended.load(Ordering::Acquire) {
            return Ok(());
        }
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let mut lease = self.lock_supervision_lease()?;
        let result = self.recover_suspended(&db, &mut lease);
        if let Err(error) = &result {
            self.storage_failure(&mut lease, error);
        }
        result
    }
    /// Called only with DB then shared lease locked. The shared flag is checked
    /// again here so concurrent siblings do not each scan a recovered origin.
    fn recover_suspended(
        &self,
        db: &Connection,
        lease: &mut SupervisionLease,
    ) -> anyhow::Result<()> {
        self.recover_suspended_with_access(db, lease, None, StoreOperationClass::Business)
    }
    fn recover_suspended_with_access(
        &self,
        db: &Connection,
        lease: &mut SupervisionLease,
        access: Option<&HumanSourceAccess<'_>>,
        class: StoreOperationClass,
    ) -> anyhow::Result<()> {
        // Also fence a preopened legacy handle before looking at a suspension
        // flag or publishing any recovered caches/activation witness.
        review_access::validate_context(self, db, access, class)?;
        if !self.supervision_lease.suspended.load(Ordering::Acquire) {
            return Ok(());
        }
        anyhow::ensure!(
            self.permanently_healthy() && !lease.invalidated,
            "audit integrity failure is latched; recovery required"
        );
        let tx = db.unchecked_transaction()?;
        review_access::validate_context(self, &tx, access, class)?;
        self.backing.verify(&tx)?;
        let before_snapshot: i64 = tx.pragma_query_value(None, "data_version", |r| r.get(0))?;
        validate_schema(&tx, false)?;
        Self::validate_metadata(&tx, &self.tenant_id)?;
        anyhow::ensure!(Self::check_integrity(&tx)?.valid, "audit integrity failure");
        control::validate(&tx)?;
        lease.verify(&self.backing, &tx)?;
        let head = audit_head(&tx)?;
        anyhow::ensure!(
            head.0 >= lease.floor.0,
            "audit head regressed during recovery"
        );
        let current_version: i64 = tx.pragma_query_value(None, "data_version", |r| r.get(0))?;
        let checked_changes = tx.total_changes();
        let mut capacity = self.lock_access_capacity(access)?;
        review_access::validate_context(self, &tx, access, class)?;
        let prefix = capacity.prepare(&tx)?;
        let positive_bindings = Arc::new(metadata::PositiveBindings::load(&tx)?);
        self.backing.verify(&tx)?;
        review_access::validate_context(self, &tx, access, class)?;
        // All fallible validation is complete on one stable snapshot. The
        // permanent latches are never reset, and activation comes exclusively
        // from a known committed transaction retained by this live origin.
        capacity.publish(prefix);
        self.supervision_lease
            .positive_bindings
            .store(positive_bindings);
        lease.floor = head;
        if std::mem::take(&mut lease.pending_activation) {
            lease.active = true;
        }
        if before_snapshot == current_version {
            self.validated_data_version
                .store(current_version, Ordering::Release);
        }
        self.validated_total_changes
            .store(checked_changes, Ordering::Release);
        self.supervision_lease
            .suspended
            .store(false, Ordering::Release);
        Ok(())
    }
    fn require_live_legacy(&self, db: &Connection) -> anyhow::Result<()> {
        let result = review_access::require_legacy(db);
        if let Err(error) = &result
            && error.is::<review_access::UnknownAuditSchema>()
        {
            let mut lease = self.lock_supervision_lease()?;
            self.storage_failure(&mut lease, error);
        }
        result
    }
    fn verify_storage(&self, db: &Connection) -> anyhow::Result<()> {
        self.require_live_legacy(db)?;
        let mut lease = self.lock_supervision_lease()?;
        let result = (|| {
            self.recover_suspended(db, &mut lease)?;
            self.backing.verify(db)?;
            lease.verify(&self.backing, db)
        })();
        if let Err(error) = &result {
            self.storage_failure(&mut lease, error);
        }
        result
    }
    fn verify_diagnostic_backing(&self, db: &Connection) -> anyhow::Result<()> {
        self.require_live_legacy(db)?;
        let mut lease = self.lock_supervision_lease()?;
        let result = (|| {
            // A permanently fenced Store remains readable for diagnosis, but
            // diagnosis must never restore its authority.
            if self.permanently_healthy() {
                self.recover_suspended(db, &mut lease)?;
            }
            self.backing.verify(db)
        })();
        if let Err(error) = &result {
            self.storage_failure(&mut lease, error);
        }
        result
    }
    fn prepare_event(&self, event: Event) -> anyhow::Result<(Event, String)> {
        anyhow::ensure!(
            self.permanently_healthy(),
            "audit integrity failure is latched; recovery required"
        );
        event_codec::prepare_event(&self.redactor, event)
    }
    pub fn append(&self, event: Event) -> anyhow::Result<StoredEvent> {
        event_codec::require_ordinary_kind(&event.kind)?;
        let (event, payload) = self.prepare_event(event)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let stored =
            self.append_with_capacity_class(&tx, event, payload, capacity::Class::OrdinaryWrite)?;
        self.commit_checked(tx)?;
        Ok(stored)
    }

    /// Append ordinary events in one transaction and return their original or
    /// new receipts in request order, after the checked commit.
    pub fn append_batch(&self, events: Vec<Event>) -> anyhow::Result<Vec<StoredEvent>> {
        anyhow::ensure!(
            !events.is_empty() && events.len() <= MAX_EVENT_BATCH,
            "event batch size is outside its bound"
        );
        let mut prepared = Vec::with_capacity(events.len());
        for event in events {
            event_codec::require_ordinary_kind(&event.kind)?;
            prepared.push(self.prepare_event(event)?);
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let mut stored = Vec::with_capacity(prepared.len());
        for (event, payload) in prepared {
            match self.append_with_capacity_class(
                &tx,
                event,
                payload,
                capacity::Class::OrdinaryWrite,
            ) {
                Ok(row) => stored.push(row),
                Err(failure) => {
                    tx.transaction.rollback()?;
                    return Err(if failure.is::<EventIdConflict>() {
                        BatchConflict.into()
                    } else {
                        failure
                    });
                }
            }
        }
        self.commit_checked(tx)?;
        Ok(stored)
    }

    /// Trusted runner delivery only: the gateway authenticates before parsing
    /// the body. This entry point rechecks the closed kind set at commit; no
    /// Event field can promote ordinary ingestion to the control reserve.
    pub(crate) fn append_control_event(&self, event: Event) -> anyhow::Result<StoredEvent> {
        anyhow::ensure!(
            control_delivery_kind(&event.kind),
            "unsupported control delivery kind"
        );
        let resume = event.kind == RETIREMENT_RESUME_EVENT_KIND;
        if resume {
            retirement_resume_record(&event)?;
        }
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            control_delivery_kind(&event.kind),
            "control delivery kind changed during preparation"
        );
        // Mask before persistence and refuse a redaction collision that would
        // change any canonical binding or the closed request attribution.
        if resume {
            retirement_resume_record(&event)?;
        }
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
        self.begin_checked_with_access(db, None, StoreOperationClass::Business)
    }
    pub(crate) fn begin_checked_scoped<'a>(
        &'a self,
        db: &'a mut Connection,
        access: &'a HumanSourceAccess<'a>,
        class: StoreOperationClass,
    ) -> anyhow::Result<CheckedTransaction<'a>> {
        access.check(class)?;
        // Caller retains its exclusive Store.db guard for the transaction.
        // Only this borrowed handle is configured; no shared timeout is reset
        // outside that guard, and no owner gate is reacquired here.
        db.busy_timeout(std::time::Duration::from_millis(100))?;
        self.begin_checked_with_access(db, Some(access), class)
    }
    fn begin_checked_with_access<'a>(
        &'a self,
        db: &'a mut Connection,
        source_access: Option<&'a HumanSourceAccess<'a>>,
        operation_class: StoreOperationClass,
    ) -> anyhow::Result<CheckedTransaction<'a>> {
        #[cfg(test)]
        self.read_test_observer
            .record(read_test_observer::Call::BeginChecked);
        // db has already been acquired by the caller. Validate after that wait
        // and before lease/recovery/cache/replay, including on legacy handles.
        if source_access.is_some() {
            review_access::validate_context(self, db, source_access, operation_class)?;
        } else {
            self.require_live_legacy(db)?;
        }
        let mut lease = self.lock_access_lease(source_access)?;
        if let Err(error) =
            review_access::validate_context(self, db, source_access, operation_class)
        {
            if error.is::<review_access::UnknownAuditSchema>() {
                self.storage_failure(&mut lease, &error);
            }
            return Err(error);
        }
        if let Err(error) =
            self.recover_suspended_with_access(db, &mut lease, source_access, operation_class)
        {
            if !review_access::admission_refusal(&error, source_access.is_some()) {
                self.storage_failure(&mut lease, &error);
            }
            return Err(error);
        }
        anyhow::ensure!(
            self.healthy() && !lease.invalidated,
            "audit integrity failure is latched; recovery required"
        );
        if let Err(error) = self.backing.verify(db) {
            self.storage_failure(&mut lease, &error);
            return Err(error);
        }
        // Retain the same connection for the postcommit check. The exclusive
        // &mut entry and the caller's DB guard still prevent a nested caller;
        // new_unchecked additionally refuses an already-open transaction.
        let connection: &'a Connection = db;
        let tx = Transaction::new_unchecked(connection, rusqlite::TransactionBehavior::Immediate)?;
        if let Err(error) =
            review_access::validate_context(self, &tx, source_access, operation_class)
        {
            if error.is::<review_access::UnknownAuditSchema>() {
                self.storage_failure(&mut lease, &error);
            }
            return Err(error);
        }
        if let Err(error) = self.backing.verify(&tx) {
            self.storage_failure(&mut lease, &error);
            return Err(error);
        }
        if let Err(error) = lease.verify(&self.backing, &tx) {
            self.storage_failure(&mut lease, &error);
            return Err(error);
        }
        // data_version changes for commits made by another SQLite connection,
        // including DDL. The write transaction keeps that validated snapshot
        // stable until this append is committed, without rescanning our own
        // unchanged chain for every event.
        let current_version: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let changed = current_version != self.validated_data_version.load(Ordering::Acquire)
            || tx.total_changes() != self.validated_total_changes.load(Ordering::Acquire);
        let verification = (|| -> anyhow::Result<()> {
            // Schema checks also cover DDL/PRAGMA issued on this connection,
            // which need not advance either SQLite data_version or total_changes.
            validate_schema(&tx, false)?;
            if changed {
                Self::validate_metadata(&tx, &self.tenant_id)?;
                anyhow::ensure!(Self::check_integrity(&tx)?.valid, "audit integrity failure");
                control::validate(&tx)?;
            }
            // integrity() can already have validated an external commit. The
            // independent capacity anchor still has to advance in that case.
            let mut capacity = self.lock_access_capacity(source_access)?;
            review_access::validate_context(self, &tx, source_access, operation_class)?;
            let prefix = capacity.prepare(&tx)?;
            review_access::validate_context(self, &tx, source_access, operation_class)?;
            capacity.publish(prefix); // No new event exists in this transaction yet.
            Ok(())
        })();
        if let Err(error) = verification {
            if !review_access::admission_refusal(&error, source_access.is_some()) {
                self.storage_failure(&mut lease, &error);
            }
            return Err(error);
        }
        if changed {
            // This snapshot contains only previously committed data: no
            // caller has received the transaction or attempted a mutation.
            // Publish before its validation marker even if the caller later
            // returns a business error and rolls this transaction back.
            let bindings = Arc::new(metadata::PositiveBindings::load(&tx)?);
            review_access::validate_context(self, &tx, source_access, operation_class)?;
            self.supervision_lease.positive_bindings.store(bindings);
        }
        review_access::validate_context(self, &tx, source_access, operation_class)?;
        self.validated_data_version
            .store(current_version, Ordering::Release);
        Ok(CheckedTransaction {
            transaction: tx,
            connection,
            origin: self,
            lease,
            source_access,
            operation_class,
            activate_supervision: false,
            positive_bindings: None,
        })
    }
    fn commit_checked(&self, tx: CheckedTransaction<'_>) -> anyhow::Result<()> {
        // A foreign CheckedTransaction is an argument refusal, before touching
        // either Store's health or the actual origin's retained lease.
        if !std::ptr::eq(self, tx.origin) {
            return Err(review_authority::ReviewAuthorityFailure::ForeignTransaction.into());
        }
        review_access::validate_context(self, tx.connection, tx.source_access, tx.operation_class)?;
        let CheckedTransaction {
            transaction,
            connection,
            origin: _,
            mut lease,
            source_access,
            operation_class,
            activate_supervision,
            positive_bindings,
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
            let checked_changes = transaction.total_changes();
            let mut capacity = self.lock_access_capacity(source_access)?;
            let capacity_prefix = capacity.prepare(&transaction)?;
            let positive_bindings = positive_bindings.map(Arc::new);
            review_access::validate_context(self, &transaction, source_access, operation_class)?;
            transaction.commit()?;
            // Publish committed positive ownership before any fallible
            // filesystem observation. This is not an authority/ACK: a failed
            // observation still suspends the source until full recovery.
            if let Some(bindings) = positive_bindings {
                self.supervision_lease.positive_bindings.store(bindings);
            }
            // A known committed mutation must advance the rollback witness
            // even if its pathname check cannot run. The shared guard still
            // closes all authority; activation and counters remain unpublished.
            lease.floor = next_head;
            lease.namespace = namespace;
            lease.pending_activation = activate_supervision;
            // COMMIT is known: its floor is already retained even if the epoch
            // expired during COMMIT or the source check now refuses the ACK.
            // Use the exact connection, never a synthetic postcommit tx.
            review_access::validate_context(self, connection, source_access, operation_class)?;
            self.backing.verify_names()?;
            capacity.publish(capacity_prefix);
            // The shared guard still covers this publication, before an ACK or EmitOnce.
            if std::mem::take(&mut lease.pending_activation) {
                lease.active = true;
            }
            // Publish only our checked changes. In particular, do not read and
            // publish a new data_version after commit: a concurrent external
            // commit must remain unvalidated for the next transaction.
            self.validated_total_changes
                .store(checked_changes, Ordering::Release);
            Ok(())
        })();
        if let Err(error) = &result {
            if !review_access::admission_refusal(error, source_access.is_some()) {
                self.storage_failure(&mut lease, error);
            }
        }
        result
    }
    /// Logical clock failures happen before publication and never latch Store.
    fn commit_authorized(
        &self,
        tx: CheckedTransaction<'_>,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<()> {
        review_access::validate_context(self, tx.connection, tx.source_access, tx.operation_class)?;
        workload::validate_fresh_control_time(&tx, clock)?;
        self.commit_checked(tx)
    }
    fn append_authorized_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<StoredEvent> {
        // Runner grant/binding methods reach this branch; worker sends/ACKs
        // and broker heartbeats use the separate workload-class appender.
        self.append_with_authorized_capacity_class_at(
            tx,
            event,
            payload,
            capacity::Class::ControlWrite,
            clock,
            chrono::Utc::now(),
        )
    }
    fn append_authorized_identity_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_authorized_capacity_class_at(
            tx,
            event,
            payload,
            capacity::Class::IdentityMaintenance,
            clock,
            chrono::Utc::now(),
        )
    }
    fn append_authorized_workload_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        principal: capacity::WorkloadAuditPrincipalV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_authorized_capacity_class_at(
            tx,
            event,
            payload,
            capacity::Class::Workload(principal),
            clock,
            chrono::Utc::now(),
        )
    }
    fn append_with_authorized_capacity_class_at(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        class: capacity::Class<'_>,
        clock: &workload::AuthorizedControlTimeV1,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<StoredEvent> {
        workload::validate_fresh_control_time_at(tx, clock, received_at)?;
        self.append_with_capacity_class_at(tx, event, payload, class, received_at)
    }
    fn append_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
    ) -> anyhow::Result<StoredEvent> {
        // All callers construct a typed control record, a decision, an offline
        // genesis, or come from append_control_event's closed runner contract.
        // Public ingestion deliberately uses OrdinaryWrite directly instead.
        self.append_with_capacity_class(tx, event, payload, capacity::Class::ControlWrite)
    }
    fn append_identity_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_capacity_class(tx, event, payload, capacity::Class::IdentityMaintenance)
    }
    #[cfg(test)]
    fn append_workload_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        principal: capacity::WorkloadAuditPrincipalV1,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_capacity_class(tx, event, payload, capacity::Class::Workload(principal))
    }
    fn append_human_in_transaction(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        principal: &crate::identity::PrincipalRefV1,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_capacity_class(tx, event, payload, capacity::Class::HumanRead(principal))
    }
    fn append_with_capacity_class(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        class: capacity::Class<'_>,
    ) -> anyhow::Result<StoredEvent> {
        self.append_with_capacity_class_at(tx, event, payload, class, chrono::Utc::now())
    }
    fn append_with_capacity_class_at(
        &self,
        tx: &Transaction<'_>,
        event: Event,
        payload: String,
        class: capacity::Class<'_>,
        received_at: chrono::DateTime<chrono::Utc>,
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
                EventIdConflict
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
        // Capacity counts the events physically stored, not the chain length.
        let count = u64::try_from(retention::ledger::stored_events(tx, last)?)
            .context("invalid audit event count")?;
        if retention::classify::classify(&event.kind)
            == retention::classify::EventClass::Reclaimable(retention::ReclaimClassV1::Ordinary)
        {
            if let Some(floor) = retention::ledger::latest_floor(tx)? {
                // An event declared at or before the archived horizon could be a
                // retry of a reclaimed event whose identifier is no longer live.
                let declared = chrono::DateTime::parse_from_rfc3339(&event.timestamp)?;
                let archived = chrono::DateTime::parse_from_rfc3339(&floor)?;
                if declared <= archived {
                    return Err(retention::RetentionFailure::new(
                        retention::failure::code::EVENT_PREDATES_ARCHIVE,
                        "an ordinary event declared at or before the archived horizon is refused",
                    )
                    .with("archived_floor", floor)
                    .into());
                }
            }
        }
        if let capacity::Class::EgressTerminal(reservation) = &class {
            reservation.validate(tx, &event)?;
        }
        // All appenders, including control and identity maintenance, leave
        // every other active head's durable terminal reservation untouched.
        let available = egress::protected_capacity(
            tx,
            AuditCapacityV1 {
                events: self.max_events,
                bytes: self.physical_bytes,
            },
        )?;
        if available.events < self.max_events && count >= available.events {
            return Err(egress::EgressStoreFailure::AuditReservationExhausted.into());
        }
        anyhow::ensure!(
            count < available.events,
            "audit capacity exhausted; preserve audit and contact operator"
        );
        let used: i64 = tx.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))?;
        let used_bytes = u64::try_from(used).context("invalid audit byte usage")?;
        let payload_bytes = u64::try_from(payload.len()).context("audit payload too large")?;
        if available.bytes < self.physical_bytes
            && used_bytes
                .checked_add(payload_bytes)
                .is_none_or(|next| next > available.bytes)
        {
            return Err(egress::EgressStoreFailure::AuditReservationExhausted.into());
        }
        anyhow::ensure!(
            used_bytes
                .checked_add(payload_bytes)
                .is_some_and(|next| next <= available.bytes),
            "audit byte capacity exhausted"
        );
        let schema: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // During the explicit5→6 migration the projection is created/seeded
        // before its genesis events, while user_version still correctly is5.
        let project_time = [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema)
            || (schema == 5 && workload::time_floors_present(tx)?);
        if [5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema) {
            let mut cache = self
                .supervision_lease
                .capacity
                .lock()
                .map_err(|_| anyhow::anyhow!("audit capacity cache lock poisoned"))?;
            let pending = cache.prepare(tx)?;
            let physical = available;
            let total = AuditCapacityV1 {
                events: count,
                bytes: used_bytes,
            };
            let cost = AuditCapacityV1 {
                events: 1,
                bytes: payload_bytes,
            };
            if let capacity::Class::Workload(principal) = &class {
                anyhow::ensure!(
                    [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema),
                    "workload audit requires schema6"
                );
                let event_principal = capacity::workload_identity(&event)?
                    .context("workload event attribution missing")?;
                anyhow::ensure!(
                    &event_principal == principal,
                    "workload audit class differs from typed actor"
                );
                cache.admit_workload(
                    &pending,
                    &self.capacity_policy,
                    &self.workload_audit_policy,
                    physical,
                    total,
                    cost,
                    principal,
                )?;
            } else {
                cache.admit(
                    &pending,
                    &self.capacity_policy,
                    available,
                    AuditCapacityV1 {
                        events: count,
                        bytes: used_bytes,
                    },
                    AuditCapacityV1 {
                        events: 1,
                        bytes: payload_bytes,
                    },
                    class,
                )?;
            }
        }
        let received = received_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seq = last.checked_add(1).context("audit sequence overflow")?;
        let stored_payload_bytes =
            i64::try_from(payload_bytes).context("audit payload too large")?;
        let next_used = used
            .checked_add(stored_payload_bytes)
            .context("audit byte usage overflow")?;
        let hash = digest(seq, &received, &prev, &payload);
        let decision = event.decision.as_ref().map(|d| d.outcome.as_str());
        let mutation = (|| -> anyhow::Result<()> {
            let inserted = tx.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind,agent_id,sandbox_id,project_id,decision) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![seq,event.id,received,prev,hash,payload,event.kind,event.agent_id,event.sandbox_id,event.project_id,decision])?;
            let updated = tx.execute(
                "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
                [stored_payload_bytes],
            )?;
            if project_time {
                workload::advance_time_floor(tx, seq, &received)?;
            }
            let persisted: Option<(i64, String)> = tx.query_row("SELECT seq,payload FROM events WHERE id=?1 AND received_at=?2 AND prev_hash=?3 AND hash=?4", params![event.id,received,prev,hash], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            let quota_after: i64 =
                tx.query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get(0))?;
            anyhow::ensure!(
                inserted == 1
                    && updated == 1
                    && persisted == Some((seq, payload.clone()))
                    && quota_after == next_used,
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
        let hash = decision_codec::validate_request(request, &self.redactor)?;
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
        let event = decision_codec::build_event(request, &hash, decision, jev)?;
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        Ok(Some(stored))
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
        decision_codec::find_receipt(events, request, hash)
    }
    pub fn integrity(&self) -> anyhow::Result<Integrity> {
        #[cfg(test)]
        self.read_test_observer
            .record(read_test_observer::Call::Integrity);
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.require_live_legacy(&db)?;
        let mut lease = self.lock_supervision_lease()?;
        let result = (|| -> anyhow::Result<Integrity> {
            if self.permanently_healthy() {
                self.recover_suspended(&db, &mut lease)?;
            }
            self.backing.verify(&db)?;
            // Keep schema, quota and event checks on one consistent snapshot,
            // even when another legitimate Store is appending concurrently.
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
            let version_before_snapshot: i64 =
                tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
            validate_schema(&tx, false)?;
            let current_version: i64 =
                tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
            let total_changes = tx.total_changes();
            let changed = !self.healthy()
                || version_before_snapshot != current_version
                || current_version != self.validated_data_version.load(Ordering::Acquire)
                || total_changes != self.validated_total_changes.load(Ordering::Acquire);
            let mut integrity = if changed {
                Self::validate_metadata(&tx, &self.tenant_id)?;
                let integrity = Self::check_integrity(&tx)?;
                if integrity.valid {
                    control::validate(&tx)?;
                }
                integrity
            } else {
                // Checked commits preserve the contiguous chain. Read its live
                // head rather than caching a count that misses our own appends.
                let (seq, hash) = audit_head(&tx)?;
                Integrity {
                    valid: true,
                    event_count: seq as u64,
                    head_hash: hash,
                    error: None,
                }
            };
            if integrity.valid {
                if let Err(error) = lease.verify_prefix(&self.backing, &tx) {
                    integrity.valid = false;
                    integrity.error = Some(error.to_string());
                }
            }
            let positive_bindings = if integrity.valid && changed {
                Some(Arc::new(metadata::PositiveBindings::load(&tx)?))
            } else {
                None
            };
            self.backing.verify(&tx)?;
            if integrity.valid {
                let head = audit_head(&tx)?;
                anyhow::ensure!(
                    head.0 >= lease.floor.0,
                    "audit head regressed during integrity check"
                );
                if let Some(bindings) = positive_bindings {
                    self.supervision_lease.positive_bindings.store(bindings);
                }
                lease.floor = head;
                // A commit racing the establishment of this read snapshot
                // must not be acknowledged as a validated newer snapshot.
                if version_before_snapshot == current_version {
                    self.validated_data_version
                        .store(current_version, Ordering::Release);
                }
                self.validated_total_changes
                    .store(total_changes, Ordering::Release);
            }
            Ok(integrity)
        })();
        match &result {
            Err(error) => self.storage_failure(&mut lease, error),
            Ok(integrity) if !integrity.valid => self.invalidate_storage(&mut lease),
            Ok(_) => {}
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
        let schema: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let query = if [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema) {
            "SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload,e.id,e.kind,e.agent_id,e.sandbox_id,e.project_id,e.decision,f.max_received_at FROM events e LEFT JOIN audit_time_floors f ON f.event_seq=e.seq ORDER BY e.seq"
        } else {
            "SELECT seq,received_at,prev_hash,hash,payload,id,kind,agent_id,sandbox_id,project_id,decision FROM events ORDER BY seq"
        };
        // A retained store has a sparse closed prefix; its segments account
        // for every reclaimed event and carry each boundary hash and floor.
        let segments = retention::ledger::segments(db)?;
        let boundary_floors: BTreeMap<i64, String> = segments
            .iter()
            .map(|segment| (segment.last_seq, segment.floor_at_last.clone()))
            .collect();
        let Some(mut chain) = retention::chain::RetainedChain::new(
            segments
                .iter()
                .map(retention::ledger::SegmentRow::bound)
                .collect(),
        ) else {
            return Ok(Integrity {
                valid: false,
                event_count: 0,
                head_hash: "0".repeat(64),
                error: Some("audit retention ledger does not tile the archived prefix".into()),
            });
        };
        let base = chain.base().0;
        let mut stmt = db.prepare(query)?;
        let mut rows = stmt.query([])?;
        let mut time_floor: Option<chrono::DateTime<chrono::Utc>> = None;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let received: String = row.get(1)?;
            let previous: String = row.get(2)?;
            let hash: String = row.get(3)?;
            let payload: String = row.get(4)?;
            let parsed = serde_json::from_str::<Event>(&payload);
            let projection_valid = if let Ok(e) = parsed {
                let decision = e.decision.map(|d| d.outcome.as_str().to_string());
                e.id == row.get::<_, String>(5)?
                    && e.kind == row.get::<_, String>(6)?
                    && e.agent_id == row.get(7)?
                    && e.sandbox_id == row.get(8)?
                    && e.project_id == row.get(9)?
                    && decision == row.get(10)?
            } else {
                false
            };
            let time_valid = if [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema) {
                let received = workload::canonical_control_time(
                    crate::broker_registry::parse_time(&received)?,
                )?;
                let stored = row.get::<_, Option<String>>(11)?;
                if seq <= base {
                    // A sparse prefix keeps its recorded floors: each covers its
                    // own receive time, never decreases, and each boundary
                    // carries the floor that its segment recorded.
                    match stored
                        .as_deref()
                        .map(crate::broker_registry::parse_time)
                        .transpose()
                    {
                        Ok(Some(floor)) => {
                            let ordered = floor >= received
                                && time_floor.is_none_or(|previous| floor >= previous)
                                && boundary_floors
                                    .get(&seq)
                                    .is_none_or(|recorded| stored.as_ref() == Some(recorded));
                            time_floor = Some(floor);
                            ordered
                        }
                        _ => false,
                    }
                } else {
                    let maximum = time_floor.map_or(received, |previous| previous.max(received));
                    time_floor = Some(maximum);
                    stored == Some(crate::broker_registry::format_time(maximum))
                }
            } else {
                true
            };
            let (verified, prefix_hash) = chain.head();
            if !chain.accept(seq, &received, &previous, &hash, &payload)
                || !projection_valid
                || !time_valid
            {
                return Ok(Integrity {
                    valid: false,
                    event_count: verified as u64,
                    head_hash: prefix_hash,
                    error: Some(format!("integrity violation at sequence {seq}")),
                });
            }
        }
        let (head, head_hash) = chain.head();
        if !chain.complete() {
            return Ok(Integrity {
                valid: false,
                event_count: head as u64,
                head_hash,
                error: Some("audit retention accounting violation".into()),
            });
        }
        if [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&schema) {
            let floors: i64 =
                db.query_row("SELECT COUNT(*) FROM audit_time_floors", [], |row| {
                    row.get(0)
                })?;
            anyhow::ensure!(
                u64::try_from(floors).ok() == Some(chain.stored()),
                "control time floor projection has unexpected rows"
            );
        }
        Ok(Integrity {
            valid: true,
            event_count: head as u64,
            head_hash,
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
        q.validated_dates()?;
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
            sql.push_str(" AND julianday(received_at)>=julianday(?)");
            args.push(s.clone().into());
        }
        if let Some(s) = &q.to {
            sql.push_str(" AND julianday(received_at)<=julianday(?)");
            args.push(s.clone().into());
        }
        if let Some(s) = &q.q {
            sql.push_str(" AND instr(lower(payload), lower(?))>0");
            args.push(s.clone().into());
        }
        sql.push_str(if q.order.as_deref() == Some("asc") {
            " ORDER BY seq ASC LIMIT ?"
        } else {
            " ORDER BY seq DESC LIMIT ?"
        });
        args.push((q.page_limit() as i64).into());
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
        // After retention the fold resumes from the snapshot of the complete
        // history through the last segment boundary.
        let (base, mut projection) = match retention::ledger::latest_snapshot(&db)? {
            Some(snapshot) => (
                snapshot.through_seq,
                entities::Projection::from_state(
                    sandboxes,
                    if sandboxes {
                        snapshot.sandboxes
                    } else {
                        snapshot.agents
                    },
                ),
            ),
            None => (0, entities::Projection::new(sandboxes)),
        };
        let mut stmt = db.prepare(if sandboxes {
            "SELECT payload FROM events WHERE sandbox_id IS NOT NULL AND seq>?1 ORDER BY seq"
        } else {
            "SELECT payload FROM events WHERE agent_id IS NOT NULL AND seq>?1 ORDER BY seq"
        })?;
        let mut rows = stmt.query([base])?;
        while let Some(row) = rows.next()? {
            let event: Event = serde_json::from_str(&row.get::<_, String>(0)?)?;
            projection.observe(&event);
        }
        Ok(projection.finish())
    }
    pub fn overview(&self) -> anyhow::Result<Value> {
        let agents = self.entities(false)?;
        let sandboxes = self.entities(true)?;
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        self.verify_storage(&db)?;
        // Counters cover the complete history: the snapshot through the last
        // retention boundary plus the live events after it. Decisions are
        // structural and never reclaimed, so pending reviews read them all.
        let (base, archived) = retention::ledger::latest_snapshot(&db)?
            .map(|snapshot| (snapshot.through_seq, snapshot.counts))
            .unwrap_or_default();
        let count = |condition: &str| -> anyhow::Result<i64> {
            Ok(db.query_row(
                &format!("SELECT count(*) FROM events WHERE ({condition}) AND seq>?1"),
                [base],
                |r| r.get(0),
            )?)
        };
        let pending:i64=db.query_row("SELECT count(*) FROM events e WHERE e.kind='security.decision' AND e.decision='review' AND NOT EXISTS(SELECT 1 FROM events later WHERE later.kind='security.decision' AND later.seq>e.seq AND json_extract(later.payload,'$.data.request_id')=json_extract(e.payload,'$.data.request_id'))",[],|r|r.get(0))?;
        let counts = overview::EventCounts {
            commands: archived.commands + count("kind='command.requested'")?,
            allowed: archived.allowed + count("kind='security.decision' AND decision='allow'")?,
            denied: archived.denied + count("kind='security.decision' AND decision='deny'")?,
            pending,
            security: archived.security + count("kind LIKE 'security.%'")?,
        };
        let sequence = db.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
            row.get(0)
        })?;
        Ok(counts.render(&agents, sandboxes.len(), sequence, self.healthy()))
    }
}

#[cfg(all(test, unix))]
mod incremental_integrity_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, Store) {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .unwrap();
        assert!(store.integrity().unwrap().valid);
        (directory, store)
    }

    #[test]
    fn checked_own_and_sibling_commits_keep_live_integrity_count_and_head() {
        let (_directory, store) = fixture();
        assert_eq!(store.integrity().unwrap().event_count, 1);
        let first = store.append(Event::new("fixture.own")).unwrap();
        let integrity = store.integrity().unwrap();
        assert!(integrity.valid);
        assert_eq!(integrity.event_count, 2);
        assert_eq!(integrity.head_hash, first.hash);
        let sibling = store.sibling().unwrap();
        let second = sibling.append(Event::new("fixture.sibling")).unwrap();
        let integrity = store.integrity().unwrap();
        assert!(integrity.valid);
        assert_eq!(integrity.event_count, 3);
        assert_eq!(integrity.head_hash, second.hash);
        let third = store
            .append(Event::new("fixture.own_after_sibling"))
            .unwrap();
        let integrity = store.integrity().unwrap();
        assert!(integrity.valid);
        assert_eq!(integrity.event_count, 4);
        assert_eq!(integrity.head_hash, third.hash);
    }

    #[test]
    fn unchecked_changes_on_the_same_connection_do_not_reuse_validation() {
        for statement in [
            "UPDATE events SET hash='tampered'",
            "UPDATE quota SET bytes=0",
            "CREATE INDEX unexpected ON events(kind)",
            "PRAGMA user_version=999",
        ] {
            let (_directory, store) = fixture();
            store.db.lock().unwrap().execute_batch(statement).unwrap();
            assert!(
                store.integrity().map_or(true, |integrity| !integrity.valid),
                "{statement}"
            );
            assert!(!store.healthy(), "{statement}");
            assert!(store.append(Event::new("fixture.after_tamper")).is_err());
        }

        let (_directory, store) = fixture();
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("PRAGMA user_version=999")
            .unwrap();
        assert!(store.healthy());
        assert!(store.append(Event::new("fixture.invalid_schema")).is_err());
        assert!(!store.healthy());
    }

    #[test]
    fn external_changes_are_not_masked_by_a_subsequent_checked_write() {
        for statement in [
            "UPDATE events SET hash='tampered'",
            "UPDATE quota SET bytes=0",
            "CREATE INDEX unexpected ON events(kind)",
        ] {
            let (directory, store) = fixture();
            let external = Connection::open(directory.path().join("audit.sqlite")).unwrap();
            external.execute_batch(statement).unwrap();
            assert!(
                store.append(Event::new("fixture.after_external")).is_err(),
                "{statement}"
            );
            assert!(!store.healthy(), "{statement}");
            assert!(
                store.integrity().map_or(true, |integrity| !integrity.valid),
                "{statement}"
            );
        }
    }
}

#[cfg(all(test, unix))]
mod redaction_attachment_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn additional_redaction_reuses_the_live_origin_and_a_covering_view() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let original = Arc::new(
            Store::open(
                &directory.path().join("audit.sqlite"),
                Redactor::new(vec![]),
                100,
            )
            .unwrap(),
        );
        let requested = Redactor::new(vec!["PrivateCanaryGHIJKLMNOPQRSTUVWXYZ".into()]);
        let attached = original.with_additional_redaction(&requested).unwrap();
        assert!(!Arc::ptr_eq(&original, &attached));
        assert!(Arc::ptr_eq(
            &original.supervision_lease,
            &attached.supervision_lease
        ));
        assert!(original.backing.identity_key() == attached.backing.identity_key());
        assert!(!original.redactor.covers(&requested));
        assert!(attached.redactor.covers(&requested));
        assert!(Arc::ptr_eq(
            &attached,
            &attached.with_additional_redaction(&requested).unwrap()
        ));
        let mut event = Event::new("fixture.redaction");
        event.data = json!({"text":"PrivateCanaryGHIJKLMNOPQRSTUVWXYZ"});
        let stored = attached.append(event).unwrap();
        let queried = original
            .events(&EventQuery {
                order: Some("asc".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(queried.last().unwrap().hash, stored.hash);
        assert_eq!(stored.event.data["text"], "[REDACTED]");
    }
}
