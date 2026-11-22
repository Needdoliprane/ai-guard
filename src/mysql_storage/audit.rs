//! Transactional audit for a prepared MySQL target.
use super::{
    MysqlCredentials, MysqlEndpoint, MysqlProbeError, broker_registry, connect_options, decision,
    driver_error, event_read::Selection, identity, open_verified, supervision, worker, workload,
};
use crate::{
    audit::{
        AuditCapacityPolicyV1, AuditCapacityV1, EventQuery, Integrity,
        RETIREMENT_RESUME_EVENT_KIND,
        capacity_policy::{PHYSICAL_BYTES, ReservedWrite},
        control_delivery_kind,
        decision_codec::{self, DecisionFailure},
        entities,
        event_codec::{digest, prepare_event},
        overview, retirement_resume_record, supervision_admission, supervision_genesis,
        worker_admission::{self, ControlRecord},
        worker_transition,
        workload::projection::Change as WorkloadChange,
    },
    broker_registry::{
        BrokerRegistryFailureV1, BrokerViewV1, registry_uuid, require as require_broker,
    },
    control::{
        AdmissionResultV1, AdmissionV1, AllocationV1, ControlFailure, MutationReceiptV1,
        MutationResultV1, QuotaPolicyV1, QuotaUsageV1, SupervisedAdmissionV1, WorkerMutationV1,
        WorkerV1, invalid, uuid,
    },
    identity::IdentityFailureV1,
    model::{Decision, DecisionRequest, Event, Outcome, StoredEvent},
    policy::Policy,
    redaction::Redactor,
    tenancy::validate_tenant_id,
    workload::{
        WorkloadBindingIssueV1, WorkloadBindingReceiptV1, WorkloadBindingRevocationV1,
        WorkloadDigestOwnerV1, WorkloadFailureV1, workload_valid,
    },
};
use anyhow::{Context, Result, anyhow, ensure};
use arc_swap::ArcSwap;
use chrono::{DateTime, SecondsFormat, Utc};
use mysql_async::{
    Conn, IsolationLevel, Opts, Row, Transaction, TxOpts, from_row_opt, prelude::Queryable,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::Mutex,
    time::{Instant, timeout_at},
};
use uuid::Uuid;

pub use super::supervision::MysqlSupervisionOrigin;

pub(crate) const DEADLINE: Duration = Duration::from_secs(8);
// The gateway's CredentialIndex admits one runner and one viewer for at most 256 tenants.
const MAX_CONTROL_COLLISION_CANDIDATES: usize = 512;
const GENESIS: &str = "control.mysql.audit.genesis";
const STATE: &str = "SELECT schema_version,tenant_id,namespace_id,head_seq,head_hash,payload_bytes FROM ai_guard_audit_state WHERE singleton=1";
const LOCK_STATE: &str = "SELECT schema_version,tenant_id,namespace_id,head_seq,head_hash,payload_bytes FROM ai_guard_audit_state WHERE singleton=1 FOR UPDATE";
const EVENTS: &str = "SELECT e.seq,e.event_id,e.received_at,e.prev_hash,e.hash,e.payload,
    d.request_id,d.human_approved,d.request_hash,
    EXISTS(SELECT 1 FROM ai_guard_worker w WHERE w.request_id=d.request_id)
    FROM ai_guard_audit_event e
    LEFT JOIN ai_guard_audit_decision d ON e.seq=d.event_seq ORDER BY e.seq ASC";
const EVENT_BY_ID: &str = "SELECT seq,event_id,received_at,prev_hash,hash,payload FROM ai_guard_audit_event WHERE event_id=?";
const FIRST_EVENT: &str =
    "SELECT seq,event_id,received_at,prev_hash,hash,payload FROM ai_guard_audit_event WHERE seq=1";

/// A failed or timed-out COMMIT cannot prove rollback.
#[derive(Debug)]
pub struct CommitUncertain;
impl fmt::Display for CommitUncertain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MySQL audit commit outcome is unknown; reconcile the same operation")
    }
}
impl std::error::Error for CommitUncertain {}

#[derive(Debug)]
struct AuditChainMismatch {
    prefix: AuditHead,
    sequence: i64,
    reason: &'static str,
}

impl fmt::Display for AuditChainMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for AuditChainMismatch {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditHead {
    pub seq: i64,
    pub hash: String,
    pub payload_bytes: u64,
}

impl AuditHead {
    fn empty() -> Self {
        Self {
            seq: 0,
            hash: "0".repeat(64),
            payload_bytes: 0,
        }
    }

    fn extended(&self, received: &str, payload: &str) -> Result<Self> {
        let seq = self.seq.checked_add(1).context("audit sequence overflow")?;
        let payload_bytes = self
            .payload_bytes
            .checked_add(payload.len() as u64)
            .filter(|bytes| *bytes <= PHYSICAL_BYTES)
            .context("audit byte capacity exhausted")?;
        Ok(Self {
            seq,
            hash: digest(seq, received, &self.hash, payload),
            payload_bytes,
        })
    }
}

struct Binding {
    tenant: Option<String>,
    namespace: Option<String>,
    head: AuditHead,
}

pub(super) struct EncodedEvent {
    pub(super) stored: StoredEvent,
    payload: String,
    received: DateTime<Utc>,
}

pub(super) fn decode_row<T: mysql_async::prelude::FromRow>(row: Row) -> Result<T> {
    from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL audit row"))
}

pub(super) type EventColumns = (i64, String, String, String, String, String);
type ProjectedEventColumns = (
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<u8>,
    Option<String>,
    u8,
);

pub(super) fn decode_event(row: Row) -> Result<EncodedEvent> {
    decode_event_columns(decode_row(row)?)
}

pub(super) fn decode_event_columns(
    (seq, id, received_at, prev_hash, hash, payload): EventColumns,
) -> Result<EncodedEvent> {
    ensure!(payload.len() <= 131_072, "oversized MySQL audit event");
    let received = DateTime::parse_from_rfc3339(&received_at)
        .map_err(|_| anyhow!("invalid MySQL audit receipt timestamp"))?
        .with_timezone(&Utc);
    ensure!(
        received.to_rfc3339_opts(SecondsFormat::Millis, true) == received_at,
        "noncanonical MySQL audit receipt timestamp"
    );
    let event: Event =
        serde_json::from_str(&payload).map_err(|_| anyhow!("invalid stored audit event"))?;
    event
        .validate()
        .map_err(|_| anyhow!("invalid stored audit event"))?;
    ensure!(event.id == id, "MySQL event identity mismatch");
    Ok(EncodedEvent {
        stored: StoredEvent {
            seq,
            received_at,
            prev_hash,
            hash,
            event,
        },
        payload,
        received,
    })
}

async fn binding(tx: &mut Transaction<'_>, query: &str) -> Result<Binding> {
    let row = tx
        .query_first::<Row, _>(query)
        .await
        .map_err(driver_error)?
        .context("MySQL audit schema was not prepared")?;
    let (version, tenant, namespace, seq, hash, payload_bytes): (
        u16,
        Option<String>,
        Option<String>,
        i64,
        String,
        u64,
    ) = decode_row(row)?;
    ensure!(version == 1, "unsupported MySQL audit schema version");
    Ok(Binding {
        tenant,
        namespace,
        head: AuditHead {
            seq,
            hash,
            payload_bytes,
        },
    })
}

async fn require_transactional_schema(tx: &mut Transaction<'_>) -> Result<()> {
    // Keep the append and projection tables under the transaction's metadata locks.
    tx.query_drop("SELECT seq FROM ai_guard_audit_event WHERE seq=0")
        .await
        .map_err(driver_error)?;
    tx.query_drop("SELECT event_seq FROM ai_guard_audit_decision WHERE event_seq=0")
        .await
        .map_err(driver_error)?;
    for probe in [
        "SELECT event_seq FROM ai_guard_worker WHERE event_seq=0",
        "SELECT attempt_id FROM ai_guard_worker_attempt WHERE worker_id=X''",
        "SELECT event_seq FROM ai_guard_worker_mutation WHERE event_seq=0",
        "SELECT event_seq FROM ai_guard_worker_observation WHERE event_seq=0",
        "SELECT event_seq FROM ai_guard_worker_allocation WHERE event_seq=0",
        "SELECT genesis_seq FROM ai_guard_supervision_meta WHERE singleton=1",
        "SELECT event_seq FROM ai_guard_worker_supervision WHERE event_seq=0",
    ] {
        tx.query_drop(probe).await.map_err(driver_error)?;
    }
    let row = tx.query_first::<Row, _>(
        "SELECT COUNT(*), @@GLOBAL.innodb_flush_log_at_trx_commit FROM information_schema.TABLES \
         WHERE BINARY TABLE_SCHEMA = BINARY DATABASE() AND TABLE_TYPE = 'BASE TABLE' AND ENGINE = 'InnoDB' \
         AND BINARY TABLE_NAME IN ('ai_guard_audit_state', 'ai_guard_audit_event', 'ai_guard_audit_decision',
             'ai_guard_worker', 'ai_guard_worker_attempt', 'ai_guard_worker_mutation',
             'ai_guard_worker_observation', 'ai_guard_worker_allocation', 'ai_guard_supervision_meta', 'ai_guard_worker_supervision')",
    ).await.map_err(driver_error)?.context("MySQL durability settings unavailable")?;
    let (tables, flush): (u64, u64) = decode_row(row)?;
    ensure!(
        tables == 10 && flush == 1,
        "MySQL audit requires durable InnoDB tables"
    );
    Ok(())
}

pub struct MysqlAuditStore {
    options: Opts,
    database: String,
    username: String,
    tenant: String,
    namespace: String,
    redactor: Redactor,
    max_events: i64,
    capacity_policy: AuditCapacityPolicyV1,
    confirmed: Mutex<Confirmed>,
    control_collision_candidates: BTreeMap<String, [u8; 32]>,
    known_control_collisions: ArcSwap<BTreeMap<[u8; 32], ControlCollision>>,
}

struct Confirmed {
    head: AuditHead,
    origin: Option<supervision::Witness>,
}

struct VerifiedPrefix {
    head: AuditHead,
    boundary: Option<StoredEvent>,
    identity: Option<identity::Verified>,
    workload: Option<workload::Verified>,
    broker_registry: Option<broker_registry::Verified>,
}

type ReadObserver<'a> = dyn FnMut(&StoredEvent, &str) -> Result<()> + Send + 'a;

enum WorkerRead<'a> {
    Quota(&'a QuotaPolicyV1),
    Operation(&'a str, &'a mut Option<MutationReceiptV1>),
    One(&'a str, &'a mut Option<WorkerV1>),
    Page {
        cursor: &'a str,
        limit: u32,
        workers: &'a mut Vec<WorkerV1>,
    },
    UsagePage {
        cursor: &'a str,
        limit: u32,
        allocations: &'a mut Vec<AllocationV1>,
    },
}

/// Presence includes inactive human bindings and historical workload digests.
/// A collision can only deny access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlCollision {
    Human,
    Workload,
    Both,
}

impl ControlCollision {
    fn from_presence(human: bool, workload: bool) -> Option<Self> {
        match (human, workload) {
            (true, false) => Some(Self::Human),
            (false, true) => Some(Self::Workload),
            (true, true) => Some(Self::Both),
            (false, false) => None,
        }
    }
}

/// A remembered collision is denial evidence. A cache miss establishes nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnownControlCollision {
    Known(ControlCollision),
    Unknown,
}

impl VerifiedPrefix {
    fn control_projections(&self) -> Result<(&identity::Verified, &workload::Verified)> {
        let identity = self
            .identity
            .as_ref()
            .ok_or(IdentityFailureV1::MigrationRequired)?;
        let workload = self
            .workload
            .as_ref()
            .ok_or(WorkloadFailureV1::MigrationRequired)?;
        Ok((identity, workload))
    }

    fn control_credential_presence(&self, digest: &[u8; 32]) -> Result<Option<ControlCollision>> {
        let (identity, workload) = self.control_projections()?;
        Ok(ControlCollision::from_presence(
            identity.has_local_credential_binding(digest),
            workload.has_historical_digest(digest),
        ))
    }
}

/// The authenticated technical credential collides with a human or workload
/// binding in the verified prefix of this very transaction. Nothing is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlGuardRefused(pub ControlCollision);

impl fmt::Display for ControlGuardRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("control credential is ambiguous with another identity class")
    }
}
impl std::error::Error for ControlGuardRefused {}

tokio::task_local! {
    static CONTROL_GUARD: [u8; 32];
}

/// Run one Store operation with the authenticated technical digest bound to
/// its write transaction. The collision check runs against the same verified
/// snapshot as the append, so a binding committed between an earlier presence
/// read and this write cannot be missed. A projection that cannot be verified
/// refuses the write.
pub async fn with_control_guard<F: std::future::Future>(
    digest: [u8; 32],
    operation: F,
) -> F::Output {
    CONTROL_GUARD.scope(digest, operation).await
}

enum Mutation<'a> {
    Initialize,
    InitializeSupervised,
    Append(Event),
    AppendControl {
        event: Event,
        retirement_resume: bool,
    },
    Decide {
        request: &'a DecisionRequest,
        hash: String,
        proposed: Option<(Decision, Value)>,
    },
    Admit {
        admission: &'a AdmissionV1,
        input_hash: String,
        proposal: Option<worker::Proposal<'a>>,
    },
    AdmitSupervised {
        input: supervision_admission::Input,
        input_hash: String,
        proposal: Option<worker::Proposal<'a>>,
    },
    ChangeWorker {
        request: &'a WorkerMutationV1,
        input_hash: &'a str,
    },
    Workload(WorkloadChange),
}

enum AppendKind<'a> {
    Ordinary,
    ControlDelivery {
        retirement_resume: bool,
    },
    Decision {
        request: &'a DecisionRequest,
        hash: String,
    },
    Worker(Box<ControlRecord>),
    SupervisedAdmission(Box<supervision_admission::Record>),
    IssueWorkload(Box<workload::Issuance>),
    RevokeWorkload(Box<workload::Revocation>),
}

struct PreparedWrite {
    stored: Option<StoredEvent>,
    head: AuditHead,
    replayed: bool,
}

impl MysqlAuditStore {
    pub fn new(
        endpoint: MysqlEndpoint<'_>,
        credentials: MysqlCredentials<'_>,
        tenant: &str,
        namespace: Uuid,
        redactor: Redactor,
        max_events: i64,
    ) -> Result<Self> {
        validate_tenant_id(tenant)?;
        ensure!(
            !namespace.is_nil() && max_events > 0,
            "invalid MySQL audit target or capacity"
        );
        let (options, _) = connect_options(&endpoint, credentials)?;
        Ok(Self {
            options: options.into(),
            database: endpoint.database.into(),
            username: endpoint.username.into(),
            tenant: tenant.into(),
            namespace: namespace.to_string(),
            redactor,
            max_events,
            capacity_policy: AuditCapacityPolicyV1::resolved(max_events as u64),
            confirmed: Mutex::new(Confirmed {
                head: AuditHead::empty(),
                origin: None,
            }),
            control_collision_candidates: BTreeMap::new(),
            known_control_collisions: ArcSwap::from_pointee(BTreeMap::new()),
        })
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant
    }

    pub fn redactor_union(&self, additional: &Redactor) -> Redactor {
        self.redactor.union(additional)
    }

    pub fn redactor_covers(&self, additional: &Redactor) -> bool {
        self.redactor.covers(additional)
    }

    /// Attach a masking union before the first connection. Only an unverified
    /// instance can change its codec; a verified head is never rebound.
    pub fn with_additional_redaction(mut self, additional: &Redactor) -> Result<Self> {
        ensure!(
            self.confirmed.get_mut().head.seq == 0,
            "redaction cannot change after the audit prefix was verified"
        );
        self.redactor = self.redactor.union(additional);
        Ok(self)
    }

    pub fn capacity_policy(&self) -> &AuditCapacityPolicyV1 {
        &self.capacity_policy
    }

    pub fn with_capacity_policy(mut self, policy: AuditCapacityPolicyV1) -> Result<Self> {
        policy.validate(self.max_events as u64)?;
        self.capacity_policy = policy;
        Ok(self)
    }

    /// Track only the configured runner and viewer digests, before verifying this Store.
    /// After verification, a candidate change requires a new Store.
    pub fn with_control_collision_candidates(mut self, digests: &[[u8; 32]]) -> Result<Self> {
        self.set_control_collision_candidates(digests)?;
        Ok(self)
    }

    pub(crate) fn set_control_collision_candidates(&mut self, digests: &[[u8; 32]]) -> Result<()> {
        ensure!(
            digests.len() <= MAX_CONTROL_COLLISION_CANDIDATES
                && self.confirmed.get_mut().head.seq == 0,
            "invalid MySQL control collision candidate configuration"
        );
        self.control_collision_candidates = digests
            .iter()
            .map(|digest| (hex::encode(digest), *digest))
            .collect();
        Ok(())
    }

    fn genesis_data(&self) -> Value {
        json!({"format": "mysql-audit-v1", "tenant_id": self.tenant, "namespace_id": self.namespace})
    }

    fn require_genesis(&self, event: &Event) -> Result<()> {
        ensure!(
            event.kind == GENESIS
                && event.source == "mysql-storage"
                && event.provenance == "observed"
                && event.coverage == "observed"
                && event.data == self.genesis_data(),
            "MySQL audit genesis mismatch"
        );
        Ok(())
    }

    async fn connect(&self) -> Result<Conn> {
        let (connection, _) =
            open_verified(self.options.clone(), &self.database, &self.username).await?;
        Ok(connection)
    }

    async fn verify_bound(
        &self,
        tx: &mut Transaction<'_>,
        bound: Binding,
        confirmed: &AuditHead,
        mut observer: Option<&mut ReadObserver<'_>>,
    ) -> Result<VerifiedPrefix> {
        ensure!(
            bound.tenant.as_deref() == Some(self.tenant.as_str())
                && bound.namespace.as_deref() == Some(self.namespace.as_str()),
            "MySQL audit binding mismatch"
        );
        let mut head = AuditHead::empty();
        let mut indexed = 0;
        let mut legacy_worker_events = 0;
        let mut supervised_worker_events = 0;
        let mut worker_history = worker::DecisionHistory::new();
        let mut retains_confirmed_prefix = confirmed.seq == 0;
        let mut boundary = None;
        let metadata = supervision::metadata(tx).await?;
        let mut identity = identity::Replay::open(tx).await?;
        let mut workload = workload::Replay::open(tx).await?;
        let mut broker_registry = broker_registry::Replay::open(tx).await?;
        let mut control_time_floor = None;
        let mut result = tx
            .query_iter(EVENTS)
            .await
            .map_err(driver_error)
            .context("MySQL audit chain query failed")?;
        while let Some(row) = result.next().await.map_err(driver_error)? {
            let (
                seq,
                id,
                received,
                prev,
                hash,
                payload,
                request_id,
                phase,
                request_hash,
                worker_request,
            ): ProjectedEventColumns = decode_row(row)?;
            let EncodedEvent {
                stored,
                payload,
                received,
            } = decode_event_columns((seq, id, received, prev, hash, payload))?;
            if head.hash != stored.prev_hash {
                return Err(AuditChainMismatch {
                    prefix: head,
                    sequence: stored.seq,
                    reason: "MySQL audit previous hash mismatch",
                }
                .into());
            }
            let next = head.extended(&stored.received_at, &payload)?;
            if next.seq != stored.seq || next.hash != stored.hash {
                return Err(AuditChainMismatch {
                    prefix: head,
                    sequence: stored.seq,
                    reason: "MySQL audit chain mismatch",
                }
                .into());
            }
            head = next;
            if let Some(observer) = observer.as_deref_mut() {
                observer(&stored, &payload)?;
            }
            if head.seq == 1 {
                self.require_genesis(&stored.event)?;
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
            } else if stored.event.kind == supervision_genesis::KIND {
                supervision_genesis::validate(&stored, &self.namespace)?;
                ensure!(
                    boundary.is_none()
                        && metadata
                            .as_ref()
                            .is_some_and(|marker| marker.matches(&stored))
                        && stored.event.coverage == "observed"
                        && request_id.is_none()
                        && phase.is_none()
                        && request_hash.is_none(),
                    "MySQL supervision boundary differs from its audit event"
                );
                boundary = Some(stored);
            } else if stored.event.kind == worker::KIND {
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                legacy_worker_events += 1;
            } else if stored.event.kind == supervision_admission::KIND {
                ensure!(
                    boundary
                        .as_ref()
                        .is_some_and(|marker| stored.seq > marker.seq),
                    "MySQL supervised worker admission requires its prior boundary"
                );
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                supervised_worker_events += 1;
            } else if workload::is_event(&stored.event.kind) {
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                workload
                    .as_mut()
                    .context("MySQL workload event requires a prepared capability")?
                    .observe(&stored, &self.tenant, &self.namespace, control_time_floor)?;
            } else if broker_registry::is_event(&stored.event.kind) {
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                broker_registry
                    .as_mut()
                    .context("MySQL broker registry event requires a prepared capability")?
                    .observe(&stored, &self.tenant, &self.namespace, control_time_floor)?;
            } else if identity::is_event(&stored.event.kind) {
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                identity
                    .as_mut()
                    .context("MySQL identity event requires a prepared capability")?
                    .observe(&stored, &self.tenant, &self.namespace)?;
            } else if stored.event.kind == RETIREMENT_RESUME_EVENT_KIND {
                ensure!(
                    request_id.is_none() && phase.is_none() && request_hash.is_none(),
                    DecisionFailure::AmbiguousHistory
                );
                retirement_resume_record(&stored.event)?;
            } else {
                let is_decision = decision::verify_projection(
                    &stored.event,
                    request_id.as_deref(),
                    phase,
                    request_hash.as_deref(),
                )?;
                indexed += u64::from(is_decision);
                if is_decision && worker_request == 1 {
                    let id = request_id.context("MySQL worker decision index is missing")?;
                    let receipts = worker_history.entry(id).or_default();
                    ensure!(receipts.len() < 2, DecisionFailure::AmbiguousHistory);
                    receipts.push(stored);
                }
            }
            if workload.is_some() || broker_registry.is_some() {
                ensure!(received.timestamp() > 0, WorkloadFailureV1::InvalidRequest);
                control_time_floor =
                    Some(control_time_floor.map_or(received, |floor| received.max(floor)));
            }
            if head.seq == confirmed.seq {
                retains_confirmed_prefix = head == *confirmed;
            }
        }
        result.drop_result().await.map_err(driver_error)?;
        let decisions = tx
            .query_first::<u64, _>("SELECT COUNT(*) FROM ai_guard_audit_decision")
            .await
            .map_err(driver_error)?;
        ensure!(
            decisions == Some(indexed),
            DecisionFailure::AmbiguousHistory
        );
        ensure!(
            head.seq > 0 && head == bound.head,
            "MySQL audit head mismatch"
        );
        ensure!(
            retains_confirmed_prefix,
            "MySQL audit does not extend its confirmed prefix"
        );
        ensure!(
            boundary.is_some() == metadata.is_some(),
            "MySQL supervision boundary is missing"
        );
        ensure!(
            boundary.is_none() || legacy_worker_events == 0,
            "legacy worker execution crosses the MySQL supervision boundary"
        );
        worker::verify_projections(
            tx,
            &self.namespace,
            legacy_worker_events + supervised_worker_events,
            &worker_history,
        )
        .await?;
        let identity = match identity {
            Some(identity) => Some(
                identity
                    .verify(tx, &self.control_collision_candidates)
                    .await?,
            ),
            None => None,
        };
        let workload = match workload {
            Some(workload) => Some(
                workload
                    .verify(
                        tx,
                        control_time_floor.context("MySQL workload audit time floor is missing")?,
                    )
                    .await?,
            ),
            None => None,
        };
        let broker_registry = match broker_registry {
            Some(registry) => Some(registry.verify(tx).await?),
            None => None,
        };
        Ok(VerifiedPrefix {
            head,
            boundary,
            identity,
            workload,
            broker_registry,
        })
    }

    async fn verify_bound_origin(
        &self,
        tx: &mut Transaction<'_>,
        bound: Binding,
        prior: Option<supervision::Witness>,
        head: &AuditHead,
        observer: Option<&mut ReadObserver<'_>>,
    ) -> Result<(VerifiedPrefix, Option<supervision::Witness>)> {
        let prefix = self.verify_bound(tx, bound, head, observer).await?;
        let origin = match prior {
            Some(prior) if prior.retained(tx, prefix.boundary.as_ref()).await? => Some(prior),
            _ => None,
        };
        Ok((prefix, origin))
    }

    pub async fn verify(&self) -> Result<AuditHead> {
        let (prefix, _, _) = self.read_verified(None, None).await?;
        Ok(prefix.head)
    }

    /// Verify the bound audit and both control identity projections in the
    /// same read-only snapshot. This checks storage prerequisites for control.
    pub async fn verify_control_projections(&self) -> Result<AuditHead> {
        let (prefix, _, _) = self.read_verified(None, None).await?;
        prefix.control_projections()?;
        Ok(prefix.head)
    }

    /// Report a row-level chain mismatch with the preceding checked prefix.
    /// Transport, schema, projection and confirmed-prefix failures remain errors.
    pub async fn integrity(&self) -> Result<Integrity> {
        let (head, error) = match self.verify().await {
            Ok(head) => (head, None),
            Err(error) => {
                let mismatch = error.downcast::<AuditChainMismatch>()?;
                let error = format!("integrity violation at sequence {}", mismatch.sequence);
                (mismatch.prefix, Some(error))
            }
        };
        Ok(Integrity {
            valid: error.is_none(),
            event_count: u64::try_from(head.seq).context("invalid MySQL audit sequence")?,
            head_hash: head.hash,
            error,
        })
    }

    /// The head of the verified tenant prefix for paging or export.
    pub async fn head(&self) -> Result<i64> {
        Ok(self.verify().await?.seq)
    }

    /// Read an audit page within a single verified MySQL snapshot.
    /// The caller remains responsible for authorization to read this tenant.
    pub async fn events(&self, query: &EventQuery) -> Result<Vec<StoredEvent>> {
        let mut selection = Selection::new(query)?;
        let mut observe = |stored: &StoredEvent, payload: &str| selection.inspect(stored, payload);
        self.read_verified(None, Some(&mut observe)).await?;
        Ok(selection.into_events())
    }

    /// Project the selected entity collection from one verified audit prefix.
    /// The caller must authorize the corresponding tenant read.
    pub async fn entities(&self, sandboxes: bool) -> Result<Vec<Value>> {
        let mut projection = entities::Projection::new(sandboxes);
        let mut observe = |stored: &StoredEvent, _: &str| {
            projection.observe(&stored.event);
            Ok(())
        };
        self.read_verified(None, Some(&mut observe)).await?;
        Ok(projection.finish())
    }

    /// Summarize entities and event counts at the same verified tenant head.
    /// A failed snapshot returns an error without publishing partial counts.
    pub async fn overview(&self) -> Result<Value> {
        let mut agents = entities::Projection::new(false);
        let mut sandboxes = entities::Projection::new(true);
        let mut counts = overview::EventCounts::default();
        let mut pending_requests = BTreeSet::new();
        let mut observe = |stored: &StoredEvent, _: &str| {
            let event = &stored.event;
            agents.observe(event);
            sandboxes.observe(event);
            counts.observe(event);

            // The verifier checks every decision's typed index and payload.
            // Keep only reviews without a later receipt for the same request.
            if event.kind == decision_codec::KIND
                && let Some(request_id) = event.data.get("request_id").and_then(Value::as_str)
                && let Some(decision) = &event.decision
            {
                match decision.outcome {
                    Outcome::Review => {
                        pending_requests.insert(request_id.to_owned());
                    }
                    Outcome::Allow | Outcome::Deny => {
                        pending_requests.remove(request_id);
                    }
                }
            }
            Ok(())
        };
        let (prefix, _, _) = self.read_verified(None, Some(&mut observe)).await?;
        counts.pending = i64::try_from(pending_requests.len())
            .context("MySQL review count exceeds audit range")?;
        Ok(counts.render(
            &agents.finish(),
            sandboxes.finish().len(),
            prefix.head.seq,
            true,
        ))
    }

    /// Presence includes inactive and expired bindings. Only a verified
    /// identity capability can establish absence.
    pub async fn has_local_credential_binding(&self, digest: &[u8; 32]) -> Result<bool> {
        let (prefix, _, _) = self.read_verified(None, None).await?;
        let projection = prefix
            .identity
            .ok_or(IdentityFailureV1::MigrationRequired)?;
        Ok(projection.has_local_credential_binding(digest))
    }

    /// Presence covers the complete verified binding history, including
    /// rotated and revoked digests. An unprepared target cannot prove absence.
    pub async fn has_workload_credential_binding(&self, digest: &[u8; 32]) -> Result<bool> {
        Ok(self.workload_digest_binding(digest).await?.is_some())
    }

    /// Resolve the immutable historical owner in this tenant's verified prefix.
    pub async fn workload_digest_binding(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<WorkloadDigestOwnerV1>> {
        let (prefix, _, _) = self.read_verified(None, None).await?;
        let projection = prefix
            .workload
            .ok_or(WorkloadFailureV1::MigrationRequired)?;
        projection.historical_digest_owner(digest)
    }

    /// Check both projections in a single fresh snapshot of this bound Store.
    /// Only two verified capabilities can establish absence.
    pub async fn verify_control_credential_presence(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<ControlCollision>> {
        let (prefix, _, _) = self.read_verified(None, None).await?;
        prefix.control_credential_presence(digest)
    }

    /// Look up a positive result from this Store's last fully verified read.
    /// A failed read leaves that result available.
    /// This opens no connection and does not wait on the transaction mutex.
    pub fn known_control_collision(&self, digest: &[u8; 32]) -> KnownControlCollision {
        self.known_control_collisions
            .load()
            .get(digest)
            .copied()
            .map_or(KnownControlCollision::Unknown, KnownControlCollision::Known)
    }

    fn publish_control_collisions(&self, prefix: &VerifiedPrefix) {
        if self.control_collision_candidates.is_empty() {
            return;
        }
        let (Some(identity), Some(workload)) = (&prefix.identity, &prefix.workload) else {
            return;
        };
        let collisions = self
            .control_collision_candidates
            .values()
            .filter_map(|digest| {
                ControlCollision::from_presence(
                    identity.knows_candidate(digest),
                    workload.has_historical_digest(digest),
                )
                .map(|collision| (*digest, collision))
            })
            .collect();
        self.known_control_collisions.store(Arc::new(collisions));
    }

    pub async fn worker_quota(&self, policy: &QuotaPolicyV1) -> Result<QuotaUsageV1> {
        policy.validate()?;
        let (_, usage, _) = self
            .read_verified(Some(WorkerRead::Quota(policy)), None)
            .await?;
        usage.context("MySQL worker quota is unavailable")
    }

    /// Read descriptive broker state from a verified administrative registry.
    pub async fn brokers_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Vec<BrokerViewV1>> {
        require_broker((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            registry_uuid(cursor)?;
        }
        let (prefix, _, _) = self.read_verified(None, None).await?;
        let registry = prefix
            .broker_registry
            .ok_or(BrokerRegistryFailureV1::MigrationRequired)?;
        registry.page(cursor.unwrap_or(""), limit)
    }

    /// Read the historical receipt for a supported worker operation in the verified prefix.
    pub async fn worker_operation(&self, id: &str) -> Result<Option<MutationReceiptV1>> {
        uuid(id)?;
        let mut receipt = None;
        self.read_verified(Some(WorkerRead::Operation(id, &mut receipt)), None)
            .await?;
        Ok(receipt)
    }

    /// Read the current worker from this tenant's verified audit snapshot.
    pub async fn worker(&self, id: &str) -> Result<Option<WorkerV1>> {
        uuid(id)?;
        let mut worker = None;
        self.read_verified(Some(WorkerRead::One(id, &mut worker)), None)
            .await?;
        Ok(worker)
    }

    /// Read up to `limit` workers ordered by canonical UUID after an exclusive cursor.
    pub async fn workers_page(&self, cursor: Option<&str>, limit: u32) -> Result<Vec<WorkerV1>> {
        invalid((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            uuid(cursor)?;
        }
        let mut workers = Vec::new();
        self.read_verified(
            Some(WorkerRead::Page {
                cursor: cursor.unwrap_or(""),
                limit,
                workers: &mut workers,
            }),
            None,
        )
        .await?;
        Ok(workers)
    }

    /// Read recorded allocations in attempt UUID order after an exclusive cursor.
    pub async fn worker_usage_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationV1>> {
        invalid((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            uuid(cursor)?;
        }
        let mut allocations = Vec::new();
        self.read_verified(
            Some(WorkerRead::UsagePage {
                cursor: cursor.unwrap_or(""),
                limit,
                allocations: &mut allocations,
            }),
            None,
        )
        .await?;
        Ok(allocations)
    }

    async fn read_verified(
        &self,
        worker_read: Option<WorkerRead<'_>>,
        observer: Option<&mut ReadObserver<'_>>,
    ) -> Result<(VerifiedPrefix, Option<QuotaUsageV1>, bool)> {
        timeout_at(Instant::now() + DEADLINE, async {
            let mut confirmed = self.confirmed.lock().await;
            let prior = confirmed.origin.take();
            let mut conn = self.connect().await?;
            let mut options = TxOpts::new();
            options
                .with_readonly(true)
                .with_isolation_level(IsolationLevel::RepeatableRead);
            let mut tx = conn
                .start_transaction(options)
                .await
                .map_err(driver_error)?;
            let bound = binding(&mut tx, STATE).await?;
            require_transactional_schema(&mut tx).await?;
            let (prefix, origin) = self
                .verify_bound_origin(&mut tx, bound, prior, &confirmed.head, observer)
                .await?;
            let usage = match worker_read {
                Some(WorkerRead::Quota(policy)) => Some(worker::usage(&mut tx, policy).await?),
                Some(WorkerRead::Operation(id, receipt)) => {
                    *receipt = worker::operation_receipt(&mut tx, id).await?;
                    None
                }
                Some(WorkerRead::One(id, selected)) => {
                    *selected = worker::find(&mut tx, id).await?;
                    None
                }
                Some(WorkerRead::Page {
                    cursor,
                    limit,
                    workers,
                }) => {
                    *workers = worker::page(&mut tx, cursor, limit).await?;
                    None
                }
                Some(WorkerRead::UsagePage {
                    cursor,
                    limit,
                    allocations,
                }) => {
                    *allocations = worker::usage_page(&mut tx, cursor, limit).await?;
                    None
                }
                None => None,
            };
            tx.rollback().await.map_err(driver_error)?;
            let retained = origin.is_some();
            confirmed.head = prefix.head.clone();
            confirmed.origin = origin;
            self.publish_control_collisions(&prefix);
            Ok((prefix, usage, retained))
        })
        .await
        .map_err(|_| MysqlProbeError::TimedOut)?
    }

    pub async fn initialize(&self) -> Result<StoredEvent> {
        self.write(Mutation::Initialize)
            .await?
            .0
            .context("MySQL audit genesis missing")
    }

    /// Establish a supervision boundary on a new, unbound, operator-prepared target.
    /// An initialized or restored target requires an independent recovery authority.
    pub async fn initialize_supervised(&self) -> Result<StoredEvent> {
        self.write(Mutation::InitializeSupervised)
            .await?
            .0
            .context("MySQL supervision genesis missing")
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Verified capabilities of this target. An operation is announced only
    /// when its MySQL port, replay and export are all implemented; the
    /// identity and workload projections are verified first. No operation of
    /// the broker supervision contract is ported to MySQL yet, so the
    /// answer is the empty set bound to the verified tenant and namespace.
    pub async fn control_capabilities(&self) -> Result<crate::control::ControlCapabilitiesV1> {
        self.verify_control_projections().await?;
        let capabilities = crate::control::ControlCapabilitiesV1 {
            version: 1,
            role: "runner".into(),
            tenant_id: self.tenant.clone(),
            control_namespace_uuid: Some(self.namespace.clone()),
            storage: crate::control::ControlStorageBackendV1::Mysql,
            operations: Vec::new(),
        };
        capabilities.validate()?;
        Ok(capabilities)
    }

    pub async fn supervision_origin(&self) -> Result<MysqlSupervisionOrigin> {
        let (prefix, _, retained) = self.read_verified(None, None).await?;
        Ok(MysqlSupervisionOrigin {
            boundary: prefix.boundary,
            fresh_origin_retained: retained,
        })
    }

    pub async fn append(&self, event: Event) -> Result<StoredEvent> {
        decision::require_ordinary(&event.kind)?;
        self.write(Mutation::Append(event))
            .await?
            .0
            .context("MySQL audit receipt missing")
    }

    /// Trusted runner delivery. Callers must authenticate the runner before
    /// parsing the request and selecting this capacity class.
    pub async fn append_control_event(&self, event: Event) -> Result<StoredEvent> {
        ensure!(
            control_delivery_kind(&event.kind),
            "unsupported control delivery kind"
        );
        let retirement_resume = event.kind == RETIREMENT_RESUME_EVENT_KIND;
        if retirement_resume {
            retirement_resume_record(&event)?;
        }
        self.write(Mutation::AppendControl {
            event,
            retirement_resume,
        })
        .await?
        .0
        .context("MySQL control audit receipt missing")
    }

    /// The trusted control evaluates policy outside the storage transaction.
    /// Both lookup and insertion verify the current chain and the exact request.
    pub async fn decision(
        &self,
        request: &DecisionRequest,
        proposed: Option<(Decision, Value)>,
    ) -> Result<Option<StoredEvent>> {
        let hash = decision_codec::validate_request(request, &self.redactor)?;
        let (stored, _) = self
            .write(Mutation::Decide {
                request,
                hash,
                proposed,
            })
            .await?;
        Ok(stored)
    }

    /// Reserve V2 worker resources and its receipt before any broker action.
    pub async fn admit_worker(
        &self,
        admission: &AdmissionV1,
        policy: &Policy,
        fresh: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> Result<AdmissionResultV1> {
        self.admission(
            admission,
            quota,
            Some(worker::Proposal {
                policy,
                fresh,
                jev,
                quota,
            }),
        )
        .await?
        .context("MySQL worker admission receipt is missing")
    }

    pub async fn replay_admission(
        &self,
        admission: &AdmissionV1,
        quota: &QuotaPolicyV1,
    ) -> Result<Option<AdmissionResultV1>> {
        self.admission(admission, quota, None).await
    }

    async fn admission(
        &self,
        admission: &AdmissionV1,
        quota: &QuotaPolicyV1,
        proposal: Option<worker::Proposal<'_>>,
    ) -> Result<Option<AdmissionResultV1>> {
        let input_hash = worker_admission::validate_input(admission, quota, &self.redactor)?;
        let (stored, replayed) = self
            .write(Mutation::Admit {
                admission,
                input_hash,
                proposal,
            })
            .await?;
        stored
            .map(|stored| {
                let record = worker::check_event(&stored.event)?;
                let mut receipt = worker_admission::admission_result(&stored, &record);
                receipt.replayed = replayed;
                Ok(receipt)
            })
            .transpose()
    }

    /// Reserve a worker under the supervision boundary established by this instance.
    pub async fn admit_supervised(
        &self,
        request: &SupervisedAdmissionV1,
        policy: &Policy,
        fresh: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> Result<AdmissionResultV1> {
        self.supervised_admission(
            request,
            quota,
            Some(worker::Proposal {
                policy,
                fresh,
                jev,
                quota,
            }),
        )
        .await?
        .context("MySQL supervised admission receipt is missing")
    }

    pub async fn replay_supervised_admission(
        &self,
        request: &SupervisedAdmissionV1,
        quota: &QuotaPolicyV1,
    ) -> Result<Option<AdmissionResultV1>> {
        self.supervised_admission(request, quota, None).await
    }

    async fn supervised_admission(
        &self,
        request: &SupervisedAdmissionV1,
        quota: &QuotaPolicyV1,
        proposal: Option<worker::Proposal<'_>>,
    ) -> Result<Option<AdmissionResultV1>> {
        let input = supervision_admission::admission_input(request, quota)?;
        let input_hash = supervision_admission::validate_input(&input, &self.redactor)?;
        let (stored, replayed) = self
            .write(Mutation::AdmitSupervised {
                input,
                input_hash,
                proposal,
            })
            .await?;
        stored
            .map(|stored| {
                let record = supervision_admission::decode(&stored)?;
                supervision_admission::result(&stored, &record, replayed)
            })
            .transpose()
    }

    /// Commit a V2 worker transition and return its original or newly committed receipt.
    pub async fn mutate_worker(&self, request: &WorkerMutationV1) -> Result<MutationResultV1> {
        let input_hash = worker_transition::validate_input(request, &self.redactor)?;
        let (stored, replayed) = self
            .write(Mutation::ChangeWorker {
                request,
                input_hash: &input_hash,
            })
            .await?;
        let stored = stored.context("MySQL worker transition receipt is missing")?;
        let record = worker::check_event(&stored.event)?;
        if replayed {
            worker_transition::replay(request, &input_hash, &record, &stored)
        } else {
            worker_transition::committed_result(&stored, &record)
        }
    }

    /// Issue, rotate or renew a binding for a caller already authorized by trusted control.
    pub async fn issue_workload_binding(
        &self,
        input: &WorkloadBindingIssueV1,
    ) -> Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.change_workload(WorkloadChange::Issue {
            input: input.clone(),
        })
        .await
    }

    /// Revoke a binding for a caller already authorized by trusted control.
    pub async fn revoke_workload_binding(
        &self,
        input: &WorkloadBindingRevocationV1,
    ) -> Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        self.change_workload(WorkloadChange::Revoke {
            input: input.clone(),
        })
        .await
    }

    async fn change_workload(&self, change: WorkloadChange) -> Result<WorkloadBindingReceiptV1> {
        let original = serde_json::to_value(&change)?;
        workload_valid(self.redactor.value(&original) == original)?;
        let (stored, replayed) = self.write(Mutation::Workload(change)).await?;
        let stored = stored.context("MySQL workload binding receipt is missing")?;
        workload::binding_receipt(&stored, replayed)
    }

    async fn write(&self, operation: Mutation<'_>) -> Result<(Option<StoredEvent>, bool)> {
        let deadline = Instant::now() + DEADLINE;
        let initialize_supervised = matches!(&operation, Mutation::InitializeSupervised);
        let mut confirmed = timeout_at(deadline, self.confirmed.lock())
            .await
            .map_err(|_| MysqlProbeError::TimedOut)?;
        let mut origin = confirmed.origin.take();
        let mut conn = timeout_at(deadline, self.connect())
            .await
            .map_err(|_| MysqlProbeError::TimedOut)??;
        let mut options = TxOpts::new();
        options
            .with_readonly(false)
            .with_isolation_level(IsolationLevel::RepeatableRead);
        let mut tx = timeout_at(deadline, conn.start_transaction(options))
            .await
            .map_err(|_| MysqlProbeError::TimedOut)?
            .map_err(driver_error)?;
        // The prefix verifier holds full worker DTOs across awaits. Keep its
        // transaction state off the stack of nested callers.
        let preparation = Box::pin(async {
            let mut workload_clock = None;
            let prepared = self
                .prepare_transaction(
                    &mut tx,
                    operation,
                    &confirmed.head,
                    &mut origin,
                    &mut workload_clock,
                )
                .await?;
            if let Some(clock) = workload_clock {
                clock.check_commit_time()?;
            }
            Ok::<_, anyhow::Error>(prepared)
        });
        let prepared = match timeout_at(deadline, preparation).await {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(error)) => {
                // Only a verified prefix can retain the witness. Restore it
                // after a confirmed rollback, including a failed projection.
                if let Ok(Ok(())) = timeout_at(deadline, tx.rollback()).await {
                    confirmed.origin = origin;
                }
                return Err(error);
            }
            Err(_) => return Err(MysqlProbeError::TimedOut.into()),
        };
        if initialize_supervised {
            let boundary = prepared
                .stored
                .as_ref()
                .context("MySQL supervision genesis missing")?;
            origin = Some(
                timeout_at(deadline, supervision::Witness::capture(&mut tx, boundary))
                    .await
                    .map_err(|_| MysqlProbeError::TimedOut)??,
            );
        }
        // No acknowledgement leaves the transaction unless COMMIT completes.
        timeout_at(deadline, tx.commit())
            .await
            .map_err(|_| CommitUncertain)?
            .map_err(|_| CommitUncertain)?;
        confirmed.head = prepared.head;
        confirmed.origin = origin;
        Ok((prepared.stored, prepared.replayed))
    }

    async fn prepare_transaction(
        &self,
        tx: &mut Transaction<'_>,
        operation: Mutation<'_>,
        confirmed: &AuditHead,
        origin: &mut Option<supervision::Witness>,
        workload_clock: &mut Option<workload::ControlClock>,
    ) -> Result<PreparedWrite> {
        let prior = origin.take();
        let bound = binding(tx, LOCK_STATE).await?;
        require_transactional_schema(tx).await?;
        if bound.tenant.is_none() && bound.namespace.is_none() {
            let supervised = match operation {
                Mutation::Initialize => false,
                Mutation::InitializeSupervised => true,
                _ => anyhow::bail!("MySQL audit binding mismatch"),
            };
            return self
                .prepare_bootstrap(tx, bound.head, confirmed, supervised)
                .await;
        }
        let (prefix, retained) = self
            .verify_bound_origin(tx, bound, prior, confirmed, None)
            .await?;
        *origin = retained;
        if let Ok(digest) = CONTROL_GUARD.try_with(|digest| *digest) {
            if let Some(collision) = prefix.control_credential_presence(&digest)? {
                return Err(ControlGuardRefused(collision).into());
            }
        }
        let mut head = prefix.head;
        let (event, kind) = match operation {
            Mutation::Initialize => {
                let row = tx
                    .query_first::<Row, _>(FIRST_EVENT)
                    .await
                    .map_err(driver_error)?
                    .context("MySQL audit genesis missing")?;
                return Ok(PreparedWrite {
                    stored: Some(decode_event(row)?.stored),
                    head,
                    replayed: true,
                });
            }
            Mutation::InitializeSupervised => return Err(ControlFailure::Conflict.into()),
            Mutation::Append(event) => (event, AppendKind::Ordinary),
            Mutation::AppendControl {
                event,
                retirement_resume,
            } => (event, AppendKind::ControlDelivery { retirement_resume }),
            Mutation::Decide {
                request,
                hash,
                proposed,
            } => {
                let existing = decision::lookup(tx, request, &hash).await?;
                let (None, Some((proposal, jev))) = (&existing, proposed) else {
                    return Ok(PreparedWrite {
                        replayed: existing.is_some(),
                        stored: existing,
                        head,
                    });
                };
                let event = decision_codec::build_event(request, &hash, proposal, jev)?;
                (event, AppendKind::Decision { request, hash })
            }
            Mutation::Admit {
                admission,
                input_hash,
                proposal,
            } => {
                ensure!(
                    prefix.boundary.is_none(),
                    ControlFailure::SupervisionInactive
                );
                let existing =
                    worker::lookup(tx, &admission.operation_id, "admit", &input_hash).await?;
                let (None, Some(proposal)) = (&existing, proposal) else {
                    return Ok(PreparedWrite {
                        replayed: existing.is_some(),
                        stored: existing,
                        head,
                    });
                };
                let record = worker::prepare(tx, admission, proposal, &self.namespace).await?;
                ensure!(
                    record.input_hash == input_hash,
                    "MySQL worker admission input differs"
                );
                let event = worker::event(&record)?;
                (event, AppendKind::Worker(Box::new(record)))
            }
            Mutation::AdmitSupervised {
                input,
                input_hash,
                proposal,
            } => {
                ensure!(prefix.boundary.is_some(), ControlFailure::MigrationRequired);
                let existing = worker::lookup(
                    tx,
                    input.operation_id(),
                    supervision_admission::ACTION,
                    &input_hash,
                )
                .await?;
                let (None, Some(proposal)) = (&existing, proposal) else {
                    return Ok(PreparedWrite {
                        replayed: existing.is_some(),
                        stored: existing,
                        head,
                    });
                };
                ensure!(origin.is_some(), ControlFailure::SupervisionInactive);
                let supervision_admission::Input::Admission(request) = &input else {
                    return Err(ControlFailure::InvalidRequest.into());
                };
                let worker =
                    worker::prepare(tx, &request.admission, proposal, &self.namespace).await?;
                let record = supervision_admission::Record::new(input, Some(worker));
                let (_, worker) = record.admission()?;
                let event = supervision_admission::event(
                    &record,
                    worker
                        .worker
                        .as_ref()
                        .map(|worker| worker.launch.session_id.as_str()),
                )?;
                (event, AppendKind::SupervisedAdmission(Box::new(record)))
            }
            Mutation::ChangeWorker {
                request,
                input_hash,
            } => {
                ensure!(
                    prefix.boundary.is_none(),
                    ControlFailure::SupervisionInactive
                );
                if let Some(stored) = worker::lookup(
                    tx,
                    &request.operation_id,
                    worker_transition::action(request),
                    input_hash,
                )
                .await?
                {
                    return Ok(PreparedWrite {
                        stored: Some(stored),
                        head,
                        replayed: true,
                    });
                }
                let previous = worker::load(tx, &request.worker_id).await?;
                let record = worker_transition::build_record(previous, request, input_hash.into())?;
                let event = worker::event(&record)?;
                (event, AppendKind::Worker(Box::new(record)))
            }
            Mutation::Workload(change) => {
                let verified = prefix
                    .workload
                    .ok_or(WorkloadFailureV1::MigrationRequired)?;
                let clock = verified.capture_control_time()?;
                if let Some(stored) = verified.replay(tx, &change).await? {
                    *workload_clock = Some(clock);
                    return Ok(PreparedWrite {
                        stored: Some(stored),
                        head,
                        replayed: true,
                    });
                }
                let next = head.seq.checked_add(1).context("audit sequence overflow")?;
                let planned = match &change {
                    WorkloadChange::Issue { input } => {
                        workload::validate_issue_worker(tx, &change, next).await?;
                        let (event, planned) = verified.plan_issue(&clock, input, next)?;
                        (event, AppendKind::IssueWorkload(Box::new(planned)))
                    }
                    WorkloadChange::Revoke { input } => {
                        let (event, planned) = verified.plan_revocation(&clock, input, next)?;
                        (event, AppendKind::RevokeWorkload(Box::new(planned)))
                    }
                };
                *workload_clock = Some(clock);
                planned
            }
        };
        let original = matches!(
            &kind,
            AppendKind::IssueWorkload(_) | AppendKind::RevokeWorkload(_)
        )
        .then(|| serde_json::to_value(&event))
        .transpose()?;
        let (event, payload) = prepare_event(&self.redactor, event)
            .map_err(|_| anyhow!("invalid MySQL audit event"))?;
        if let Some(original) = original {
            workload_valid(serde_json::to_value(&event)? == original)?;
        }
        let (decision_key, capacity_class) = match &kind {
            AppendKind::Ordinary => {
                decision::require_ordinary(&event.kind)?;
                (None, ReservedWrite::Ordinary)
            }
            AppendKind::ControlDelivery { retirement_resume } => {
                ensure!(
                    control_delivery_kind(&event.kind),
                    "control delivery kind changed during preparation"
                );
                if *retirement_resume {
                    retirement_resume_record(&event)?;
                }
                (None, ReservedWrite::Control)
            }
            AppendKind::Decision { request, hash } => {
                let expected = decision::DecisionKey::from_request(request, hash);
                ensure!(
                    decision::DecisionKey::from_event(&event)? == expected,
                    DecisionFailure::InvalidRequest
                );
                (Some(expected), ReservedWrite::Control)
            }
            AppendKind::Worker(record) => {
                worker::require_prepared_event(&event, record, &self.redactor)?;
                (None, ReservedWrite::Control)
            }
            AppendKind::SupervisedAdmission(record) => {
                supervision_admission::require_prepared(&event, record, &self.redactor)?;
                (None, ReservedWrite::Control)
            }
            AppendKind::IssueWorkload(_) => (None, ReservedWrite::Control),
            AppendKind::RevokeWorkload(_) => (None, ReservedWrite::IdentityMaintenance),
        };
        if let Some(row) = tx
            .exec_first::<Row, _, _>(EVENT_BY_ID, (&event.id,))
            .await
            .map_err(driver_error)?
        {
            let previous = decode_event(row)?;
            ensure!(
                serde_json::to_string(&previous.stored.event)? == payload,
                "event id conflict"
            );
            return Ok(PreparedWrite {
                stored: Some(previous.stored),
                head,
                replayed: true,
            });
        }
        let received_at = workload_clock
            .as_mut()
            .map(workload::ControlClock::append_time)
            .transpose()?;
        let stored = self
            .persist_event(tx, &mut head, event, payload, capacity_class, received_at)
            .await?;
        if let Some(key) = decision_key {
            key.persist(tx, head.seq).await?;
        }
        match &kind {
            AppendKind::Worker(record) => worker::persist(tx, record, head.seq).await?,
            AppendKind::SupervisedAdmission(record) => {
                worker::persist_supervised(tx, record, &stored).await?;
            }
            AppendKind::IssueWorkload(issue) => {
                issue.persist(tx, stored.seq).await?;
            }
            AppendKind::RevokeWorkload(revocation) => {
                revocation.persist(tx, stored.seq).await?;
            }
            _ => {}
        }
        tx.exec_drop(
            "UPDATE ai_guard_audit_state SET head_seq=?,head_hash=?,payload_bytes=? WHERE singleton=1",
            (head.seq, &head.hash, head.payload_bytes),
        )
        .await
        .map_err(driver_error)?;
        let persisted = binding(tx, STATE).await?;
        let prior = origin.take();
        let (durable, retained) = self
            .verify_bound_origin(tx, persisted, prior, confirmed, None)
            .await?;
        ensure!(
            durable.head == head,
            "MySQL audit persistence differs from prepared event"
        );
        *origin = retained;
        Ok(PreparedWrite {
            stored: Some(stored),
            head,
            replayed: false,
        })
    }

    async fn prepare_bootstrap(
        &self,
        tx: &mut Transaction<'_>,
        mut head: AuditHead,
        confirmed: &AuditHead,
        supervised: bool,
    ) -> Result<PreparedWrite> {
        ensure!(
            confirmed.seq == 0,
            "MySQL audit does not extend its confirmed prefix"
        );
        ensure!(
            head == AuditHead::empty(),
            "MySQL initialization requires the unbound schema"
        );
        ensure!(
            tx.query_first::<Row, _>("SELECT seq FROM ai_guard_audit_event LIMIT 1")
                .await
                .map_err(driver_error)?
                .is_none(),
            "MySQL initialization requires an empty audit"
        );
        ensure!(
            tx.query_first::<i64, _>("SELECT event_seq FROM ai_guard_audit_decision LIMIT 1")
                .await
                .map_err(driver_error)?
                .is_none(),
            "MySQL initialization requires an empty decision index"
        );
        worker::require_empty(tx).await?;
        ensure!(
            supervision::metadata(tx).await?.is_none(),
            "MySQL initialization requires empty supervision metadata"
        );
        let mut event = Event::new(GENESIS);
        event.source = "mysql-storage".into();
        event.data = self.genesis_data();
        let (event, payload) = prepare_event(&self.redactor, event)
            .map_err(|_| anyhow!("invalid MySQL audit genesis"))?;
        self.require_genesis(&event)?;
        let genesis = self
            .persist_event(tx, &mut head, event, payload, ReservedWrite::Control, None)
            .await?;
        let stored = if supervised {
            let event = supervision_genesis::event(&self.namespace, genesis.seq, &genesis.hash)?;
            let original = event.data.clone();
            let (event, payload) = prepare_event(&self.redactor, event)
                .map_err(|_| anyhow!("invalid MySQL supervision genesis"))?;
            ensure!(event.data == original, ControlFailure::InvalidRequest);
            let boundary = self
                .persist_event(tx, &mut head, event, payload, ReservedWrite::Control, None)
                .await?;
            supervision::persist(tx, &boundary).await?;
            boundary
        } else {
            genesis
        };
        tx.exec_drop(
            "UPDATE ai_guard_audit_state SET tenant_id=?,namespace_id=?,head_seq=?,head_hash=?,payload_bytes=? WHERE singleton=1",
            (&self.tenant, &self.namespace, head.seq, &head.hash, head.payload_bytes),
        ).await.map_err(driver_error)?;
        let persisted = binding(tx, STATE).await?;
        let durable = self.verify_bound(tx, persisted, confirmed, None).await?;
        ensure!(
            durable.head == head,
            "MySQL audit persistence differs from prepared event"
        );
        Ok(PreparedWrite {
            stored: Some(stored),
            head,
            replayed: false,
        })
    }

    async fn persist_event(
        &self,
        tx: &mut Transaction<'_>,
        head: &mut AuditHead,
        event: Event,
        payload: String,
        capacity_class: ReservedWrite,
        received_at: Option<DateTime<Utc>>,
    ) -> Result<StoredEvent> {
        ensure!(
            head.seq < self.max_events,
            "audit capacity exhausted; preserve audit and contact operator"
        );
        let received_at = received_at
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let next = head.extended(&received_at, &payload)?;
        let prev_hash = std::mem::replace(head, next).hash;
        self.capacity_policy.admit_write(
            AuditCapacityV1 {
                events: self.max_events as u64,
                bytes: PHYSICAL_BYTES,
            },
            AuditCapacityV1 {
                events: u64::try_from(head.seq).context("invalid MySQL audit sequence")?,
                bytes: head.payload_bytes,
            },
            capacity_class,
        )?;
        tx.exec_drop("INSERT INTO ai_guard_audit_event (seq,event_id,received_at,prev_hash,hash,payload) VALUES (?,?,?,?,?,?)",
                     (head.seq, &event.id, &received_at, &prev_hash, &head.hash, &payload)).await.map_err(driver_error)?;
        Ok(StoredEvent {
            seq: head.seq,
            received_at,
            prev_hash,
            hash: head.hash.clone(),
            event,
        })
    }
}
