//! Audited supervision for exact schemas3–10. No automatic migration.
use super::*;

pub(in crate::audit) const SCHEMA: [(&str, &str, &str); 6] = [
    (
        "table",
        "supervision_meta",
        "CREATE TABLE supervision_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),protocol TEXT NOT NULL CHECK(protocol='broker_supervision_v1'),genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "worker_supervision",
        "CREATE TABLE worker_supervision(worker_id TEXT NOT NULL PRIMARY KEY REFERENCES workers(worker_id),intent_hash TEXT NOT NULL UNIQUE,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "execution_arms",
        "CREATE TABLE execution_arms(arm_id TEXT NOT NULL PRIMARY KEY REFERENCES worker_mutations(operation_id),worker_id TEXT NOT NULL REFERENCES workers(worker_id),attempt_id TEXT NOT NULL REFERENCES worker_attempts(attempt_id),subject_key TEXT NOT NULL UNIQUE,input_hash TEXT NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "helper_claims",
        "CREATE TABLE helper_claims(helper_id TEXT NOT NULL PRIMARY KEY,arm_id TEXT NOT NULL UNIQUE REFERENCES execution_arms(arm_id),operation_id TEXT NOT NULL UNIQUE REFERENCES worker_mutations(operation_id),emitter_id TEXT NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "helper_observations",
        "CREATE TABLE helper_observations(operation_id TEXT NOT NULL PRIMARY KEY REFERENCES worker_mutations(operation_id),helper_id TEXT NOT NULL REFERENCES helper_claims(helper_id),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "helper_observations_helper",
        "CREATE INDEX helper_observations_helper ON helper_observations(helper_id,event_seq)",
    ),
];
pub(in crate::audit) const AUTO_INDEX_COUNTS: [(&str, u32); 5] = [
    ("supervision_meta", 1),
    ("worker_supervision", 3),
    ("execution_arms", 3),
    ("helper_claims", 4),
    ("helper_observations", 2),
];
const QUERIES: [(&str, &str); 5] = [
    (
        "supervision_meta",
        "SELECT singleton,protocol,genesis_seq,genesis_hash FROM supervision_meta ORDER BY singleton",
    ),
    (
        "worker_supervision",
        "SELECT worker_id,intent_hash,payload_json,event_seq FROM worker_supervision ORDER BY worker_id",
    ),
    (
        "execution_arms",
        "SELECT arm_id,worker_id,attempt_id,subject_key,input_hash,payload_json,event_seq FROM execution_arms ORDER BY arm_id",
    ),
    (
        "helper_claims",
        "SELECT helper_id,arm_id,operation_id,emitter_id,payload_json,event_seq FROM helper_claims ORDER BY helper_id",
    ),
    (
        "helper_observations",
        "SELECT operation_id,helper_id,payload_json,event_seq FROM helper_observations ORDER BY operation_id",
    ),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "action",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Input {
    Admission(Box<SupervisedAdmissionV1>),
    Bind(Box<BindLegacyIntentV1>),
    Arm(Box<ArmV1>),
    WorkerClaim(Box<ArmedStartClaimV1>),
    HelperClaim(Box<HelperStartClaimV1>),
    HelperObservation(Box<HelperObservationV1>),
    HelperCleanup(Box<HelperCleanupV1>),
    Export(Box<ExportRequestV1>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    input: Input,
    worker_record: Option<ControlRecord>,
}
pub(super) fn worker_snapshot(stored: &StoredEvent) -> anyhow::Result<Option<WorkerV1>> {
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    invalid(record.version == 2 && stored.event.kind == record.input.kind())?;
    Ok(record.worker_record.and_then(|record| record.worker))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    worker_id: String,
    intent_hash: String,
    input: Input,
}
#[derive(Clone)]
struct ArmEntry {
    request: ArmV1,
    receipt: ArmReceiptV1,
}

impl Input {
    fn operation_id(&self) -> &str {
        match self {
            Self::Admission(r) => &r.admission.operation_id,
            Self::Bind(r) => &r.operation_id,
            Self::Arm(r) => &r.operation_id,
            Self::WorkerClaim(r) => &r.operation_id,
            Self::HelperClaim(r) => &r.operation_id,
            Self::HelperObservation(r) => &r.operation_id,
            Self::HelperCleanup(r) => &r.operation_id,
            Self::Export(r) => &r.operation_id,
        }
    }
    fn action(&self) -> &'static str {
        match self {
            Self::Admission(_) => "supervised_admit",
            Self::Bind(_) => "bind_intent",
            Self::Arm(_) => "arm",
            Self::WorkerClaim(_) => "armed_start_claim",
            Self::HelperClaim(_) => "helper_start_claim",
            Self::HelperObservation(_) => "helper_observe",
            Self::HelperCleanup(_) => "helper_cleanup",
            Self::Export(_) => "export_request",
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Arm(_) => "control.execution.arm",
            _ => "control.supervision.transition",
        }
    }
    fn hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Admission(r) => r.input_hash(),
            Self::Bind(r) => r.input_hash(),
            Self::Arm(r) => r.input_hash(),
            Self::WorkerClaim(r) => r.input_hash(),
            Self::HelperClaim(r) => r.input_hash(),
            Self::HelperObservation(r) => r.input_hash(),
            Self::HelperCleanup(r) => r.input_hash(),
            Self::Export(r) => r.input_hash(),
        }
    }
    fn worker_id<'a>(&'a self, record: &'a Record) -> Option<&'a str> {
        match self {
            Self::Admission(_) => record
                .worker_record
                .as_ref()
                .and_then(|r| r.worker.as_ref())
                .map(|w| w.worker_id.as_str()),
            Self::Bind(r) => Some(&r.worker_id),
            Self::Arm(r) => Some(&r.worker_id),
            Self::WorkerClaim(r) => Some(&r.worker_id),
            Self::HelperClaim(r) => Some(&r.arm_receipt.worker_id),
            Self::HelperObservation(r) => Some(&r.arm_receipt.worker_id),
            Self::HelperCleanup(r) => Some(&r.arm_receipt.worker_id),
            Self::Export(r) => Some(&r.worker_id),
        }
    }
}
fn receipt_for(stored: &StoredEvent, record: &Record) -> anyhow::Result<MutationReceiptV1> {
    Ok(MutationReceiptV1 {
        operation_id: record.input.operation_id().into(),
        worker_id: record.input.worker_id(record).map(str::to_owned),
        revision: record
            .worker_record
            .as_ref()
            .and_then(|r| r.worker.as_ref())
            .map(|w| w.revision),
        input_hash: record.input.hash()?,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    })
}
fn mutation_row_for(
    stored: &StoredEvent,
    record: &Record,
) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    Ok(vec![
        sql_text(record.input.operation_id()),
        record
            .input
            .worker_id(record)
            .map_or(rusqlite::types::Value::Null, sql_text),
        sql_text(record.input.action()),
        sql_text(record.input.hash()?),
        rusqlite::types::Value::Integer(stored.seq),
    ])
}
fn load_record(db: &Connection, id: &str) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let stored=db.query_row("SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.operation_id=?1",[id],row_event).optional()?;
    stored
        .map(|stored: StoredEvent| {
            conflict(matches!(
                stored.event.kind.as_str(),
                "control.execution.arm" | "control.supervision.transition"
            ))?;
            let record: Record = serde_json::from_value(stored.event.data.clone())?;
            invalid(record.version == 2 && record.input.kind() == stored.event.kind)?;
            Ok((stored, record))
        })
        .transpose()
}
pub(super) fn operation_receipt(
    db: &Connection,
    id: &str,
) -> anyhow::Result<Option<MutationReceiptV1>> {
    let kind:Option<String>=db.query_row("SELECT e.kind FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.operation_id=?1",[id],|r|r.get(0)).optional()?;
    match kind.as_deref() {
        None => Ok(None),
        Some("control.worker.transition") => {
            Ok(load_operation(db, id)?.map(|(e, r)| receipt(&e, &r)))
        }
        Some(_) => load_record(db, id)?
            .map(|(e, r)| receipt_for(&e, &r))
            .transpose(),
    }
}
fn require_v3(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [3, 4, 5, 6, 7, 8, 9, 10, 11].contains(&version) {
        Ok(())
    } else {
        Err(ControlFailure::MigrationRequired.into())
    }
}
fn namespace(db: &Connection) -> anyhow::Result<String> {
    Ok(db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}
fn arm_entry(db: &Connection, id: &str) -> anyhow::Result<Option<ArmEntry>> {
    let Some((stored, record)) = load_record(db, id)? else {
        return Ok(None);
    };
    let Input::Arm(request) = record.input else {
        return Err(ControlFailure::Conflict.into());
    };
    let request = *request;
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: request.operation_id.clone(),
        worker_id: request.worker_id.clone(),
        attempt_id: request.attempt_id.clone(),
        input_hash: request.input_hash()?,
        control_namespace_uuid: namespace(db)?,
        audit_seq: stored.seq,
        audit_hash: stored.hash,
    };
    Ok(Some(ArmEntry { request, receipt }))
}
fn same_receipt(left: &MutationReceiptV1, right: &MutationReceiptV1) -> anyhow::Result<()> {
    invalid(serde_json::to_value(left)? == serde_json::to_value(right)?)
}
fn verify_receipt(db: &Connection, given: &MutationReceiptV1, before: i64) -> anyhow::Result<()> {
    let actual =
        operation_receipt(db, &given.operation_id)?.ok_or(ControlFailure::InvalidRequest)?;
    invalid(actual.audit_seq < before)?;
    same_receipt(given, &actual)
}
fn terminal_helper(observation: &RuntimeObservationV1) -> bool {
    let Some(start) = observation
        .started_at
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
    else {
        return false;
    };
    let Some(end) = observation
        .finished_at
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
    else {
        return false;
    };
    observation.running == Some(false)
        && matches!(
            observation.status,
            RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
        )
        && observation.exit_code.is_some()
        && observation.restart_count == Some(0)
        && start.timestamp() > 0
        && end >= start
}
fn terminal_parent(worker: &WorkerV1) -> bool {
    worker.state == WorkerStateV1::Terminal
        && !worker.quota_held
        && !worker.allocation_inconsistent
        && worker.last_observation.as_ref().is_some_and(|o| {
            matches!(
                classify_observation(worker, o),
                ObservationClass::Terminal(..)
            )
        })
}

/// Replay overlay state is derived from events; SQL projections are compared only afterwards.
pub(super) struct Replay {
    namespace: String,
    genesis_seq: Option<i64>,
    genesis_hash: Option<String>,
    seen_genesis: bool,
    bindings: BTreeMap<String, Binding>,
    arms: BTreeMap<String, ArmEntry>,
    claims: BTreeMap<String, HelperStartClaimV1>,
    observations: BTreeMap<String, HelperObservationV1>,
    terminals: BTreeMap<String, HelperObservationV1>,
    cleanups: BTreeMap<String, HelperCleanupV1>,
    exports: BTreeMap<String, ExportRequestV1>,
    expected: BTreeMap<&'static str, TableRows>,
}
impl Replay {
    fn current(db: &Connection) -> anyhow::Result<Self> {
        let mut state = Self::new(db, 3, namespace(db)?)?;
        state.seen_genesis = true;
        let mut stmt =
            db.prepare("SELECT payload_json FROM worker_supervision ORDER BY worker_id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let binding: Binding = serde_json::from_str(&row?)?;
            state.bindings.insert(binding.worker_id.clone(), binding);
        }
        let mut stmt = db.prepare("SELECT arm_id FROM execution_arms ORDER BY arm_id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let id = row?;
            let entry = arm_entry(db, &id)?.ok_or(ControlFailure::NotFound)?;
            state.arms.insert(id, entry);
        }
        let mut stmt = db.prepare("SELECT payload_json FROM helper_claims ORDER BY helper_id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let claim: HelperStartClaimV1 = serde_json::from_str(&row?)?;
            state.claims.insert(claim.helper_id.clone(), claim);
        }
        let mut stmt =
            db.prepare("SELECT payload_json FROM helper_observations ORDER BY event_seq")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let observation: HelperObservationV1 = serde_json::from_str(&row?)?;
            if terminal_helper(&observation.observation) {
                state
                    .terminals
                    .insert(observation.helper_id.clone(), observation.clone());
            }
            state
                .observations
                .insert(observation.operation_id.clone(), observation);
        }
        let mut stmt = db.prepare(
            "SELECT payload FROM events WHERE kind='control.supervision.transition' ORDER BY seq",
        )?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let event: Event = serde_json::from_str(&row?)?;
            let record: Record = serde_json::from_value(event.data)?;
            match record.input {
                Input::Export(r) => {
                    state.exports.insert(r.operation_id.clone(), *r);
                }
                Input::HelperCleanup(r) => {
                    state.cleanups.insert(r.helper_id.clone(), *r);
                }
                _ => {}
            }
        }
        Ok(state)
    }
    pub(super) fn new(db: &Connection, version: i64, namespace: String) -> anyhow::Result<Self> {
        let (genesis_seq, genesis_hash) = if [3, 4, 5, 6, 7, 8, 9, 10, 11].contains(&version) {
            let row: (i64, String) = db.query_row(
                "SELECT genesis_seq,genesis_hash FROM supervision_meta WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            (Some(row.0), Some(row.1))
        } else {
            (None, None)
        };
        Ok(Self {
            namespace,
            genesis_seq,
            genesis_hash,
            seen_genesis: false,
            bindings: BTreeMap::new(),
            arms: BTreeMap::new(),
            claims: BTreeMap::new(),
            observations: BTreeMap::new(),
            terminals: BTreeMap::new(),
            cleanups: BTreeMap::new(),
            exports: BTreeMap::new(),
            expected: QUERIES.iter().map(|(t, _)| (*t, BTreeMap::new())).collect(),
        })
    }
    pub(super) fn legacy_allowed(&self, seq: i64, record: &ControlRecord) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.genesis_seq.is_some_and(
                |g| seq > g && matches!(record.action.as_str(), "admit" | "start_claim")
            ),
            "legacy execution after supervision boundary"
        );
        if self.genesis_seq.is_some_and(|g| seq > g) && record.action == "create_issued" {
            let worker = record
                .worker
                .as_ref()
                .ok_or(ControlFailure::InvalidRequest)?;
            conflict(
                self.bindings.contains_key(&worker.worker_id) && self.import_verified(worker),
            )?;
        }
        Ok(())
    }
    pub(super) fn genesis(&mut self, stored: &StoredEvent) -> anyhow::Result<()> {
        let genesis: Genesis = serde_json::from_value(stored.event.data.clone())?;
        invalid(
            !self.seen_genesis
                && Some(stored.seq) == self.genesis_seq
                && Some(&stored.hash) == self.genesis_hash.as_ref()
                && genesis.version == 3
                && genesis.namespace_uuid == self.namespace
                && genesis.previous_seq == stored.seq - 1
                && genesis.previous_hash == stored.prev_hash
                && stored.event.kind == "control.supervision.genesis"
                && stored.event.source == "gateway"
                && stored.event.provenance == "observed",
        )?;
        self.seen_genesis = true;
        self.expected
            .get_mut("supervision_meta")
            .context("missing table")?
            .insert(
                "1".into(),
                vec![
                    rusqlite::types::Value::Integer(1),
                    sql_text("broker_supervision_v1"),
                    rusqlite::types::Value::Integer(stored.seq),
                    sql_text(&stored.hash),
                ],
            );
        Ok(())
    }
    fn binding(
        &mut self,
        worker: &WorkerV1,
        intent: &str,
        input: Input,
        seq: i64,
    ) -> anyhow::Result<()> {
        conflict(
            !self.bindings.contains_key(&worker.worker_id)
                && !self.bindings.values().any(|b| b.intent_hash == intent),
        )?;
        let binding = Binding {
            worker_id: worker.worker_id.clone(),
            intent_hash: intent.into(),
            input,
        };
        self.expected
            .get_mut("worker_supervision")
            .context("missing table")?
            .insert(
                worker.worker_id.clone(),
                vec![
                    sql_text(&worker.worker_id),
                    sql_text(intent),
                    sql_text(serde_json::to_string(&binding)?),
                    rusqlite::types::Value::Integer(seq),
                ],
            );
        self.bindings.insert(worker.worker_id.clone(), binding);
        Ok(())
    }
    fn parent<'a>(
        &self,
        workers: &'a BTreeMap<String, WorkerV1>,
        worker_id: &str,
    ) -> anyhow::Result<&'a WorkerV1> {
        workers
            .get(worker_id)
            .ok_or_else(|| ControlFailure::NotFound.into())
    }
    fn bound(&self, worker: &WorkerV1, intent: &str) -> anyhow::Result<()> {
        conflict(
            self.bindings
                .get(&worker.worker_id)
                .is_some_and(|b| b.intent_hash == intent),
        )
    }
    fn import_verified(&self, parent: &WorkerV1) -> bool {
        let mut found = false;
        for entry in self.arms.values() {
            if entry.request.worker_id != parent.worker_id
                || entry.request.attempt_id != parent.attempt.attempt_id
            {
                continue;
            }
            let ArmSubjectV1::Helper {
                helper_id,
                purpose: HelperPurposeV1::ImportVerification,
                ..
            } = &entry.request.subject
            else {
                continue;
            };
            found = true;
            let Some(observation) = self.terminals.get(helper_id) else {
                return false;
            };
            if !terminal_helper(&observation.observation)
                || observation.observation.exit_code != Some(0)
                || !observation.verification.as_ref().is_some_and(|report| {
                    report.expected_manifest_digest == parent.launch.workspace_manifest_digest
                        && report.observed_manifest_digest == report.expected_manifest_digest
                })
            {
                return false;
            }
        }
        found
    }
    fn checked_arm(&self, given: &ArmReceiptV1) -> anyhow::Result<&ArmEntry> {
        let entry = self
            .arms
            .get(&given.arm_id)
            .ok_or(ControlFailure::NotFound)?;
        invalid(serde_json::to_value(given)? == serde_json::to_value(&entry.receipt)?)?;
        Ok(entry)
    }
    fn permission(
        &self,
        db: &Connection,
        parent: &WorkerV1,
        arm: &ArmV1,
        before: i64,
    ) -> anyhow::Result<()> {
        let ArmSubjectV1::Helper {
            purpose,
            permission,
            ..
        } = &arm.subject
        else {
            return Ok(());
        };
        match permission.as_ref() {
            HelperPermissionV1::Admission { receipt: given } => {
                verify_receipt(db, given, before)?;
                invalid(given.operation_id == parent.admission_operation_id)?;
                if *purpose == HelperPurposeV1::FinalScan {
                    conflict(terminal_parent(parent))?;
                }
            }
            HelperPermissionV1::ImportVerification {
                import_helper_id,
                import_arm,
                import_terminal,
                manifest_digest,
            } => {
                let entry = self.checked_arm(import_arm)?;
                invalid(
                    matches!(&entry.request.subject,ArmSubjectV1::Helper {helper_id,purpose:HelperPurposeV1::Import,..} if helper_id==import_helper_id),
                )?;
                invalid(
                    entry.request.worker_id == parent.worker_id
                        && entry.request.attempt_id == parent.attempt.attempt_id
                        && manifest_digest == &parent.launch.workspace_manifest_digest,
                )?;
                let observation = self
                    .terminals
                    .get(import_helper_id)
                    .ok_or(ControlFailure::Conflict)?;
                invalid(
                    observation.operation_id == import_terminal.operation_id
                        && observation.observation.exit_code == Some(0),
                )?;
                verify_receipt(db, import_terminal, before)?;
            }
            HelperPermissionV1::Export {
                receipt: given,
                destination_ref,
                destination_intent_hash,
            } => {
                verify_receipt(db, given, before)?;
                let export = self
                    .exports
                    .get(&given.operation_id)
                    .ok_or(ControlFailure::InvalidRequest)?;
                invalid(
                    export.worker_id == parent.worker_id
                        && export.attempt_id == parent.attempt.attempt_id
                        && &export.destination_ref == destination_ref
                        && &export.destination_intent_hash == destination_intent_hash,
                )?;
                conflict(terminal_parent(parent))?;
            }
        }
        Ok(())
    }
    pub(super) fn apply(
        &mut self,
        db: &Connection,
        stored: &StoredEvent,
        workers: &BTreeMap<String, WorkerV1>,
    ) -> anyhow::Result<(Option<ControlRecord>, Vec<rusqlite::types::Value>)> {
        let record: Record = serde_json::from_value(stored.event.data.clone())?;
        invalid(
            self.seen_genesis
                && self.genesis_seq.is_some_and(|seq| stored.seq > seq)
                && record.version == 2
                && stored.event.kind == record.input.kind()
                && stored.event.source == "gateway"
                && stored.event.provenance == "observed",
        )?;
        record.input.hash()?;
        match &record.input {
            Input::Admission(request) => {
                let legacy = record
                    .worker_record
                    .as_ref()
                    .ok_or(ControlFailure::InvalidRequest)?;
                invalid(
                    legacy.action == "admit"
                        && legacy.input_hash == request.admission.input_hash()?
                        && legacy.operation_id == request.admission.operation_id,
                )?;
                if let Some(worker) = &legacy.worker {
                    invalid(
                        serde_json::to_value(&worker.launch)?
                            == serde_json::to_value(&request.admission.launch)?
                            && serde_json::to_value(&worker.receipt)?
                                == serde_json::to_value(&request.admission.receipt)?,
                    )?;
                    self.binding(
                        worker,
                        &request.intent_hash,
                        record.input.clone(),
                        stored.seq,
                    )?;
                }
            }
            Input::WorkerClaim(request) => {
                let parent = self.parent(workers, &request.worker_id)?;
                let entry = self.checked_arm(&request.arm_receipt)?;
                self.bound(parent, &entry.request.intent_hash)?;
                request.validate_for_arm(&entry.request)?;
                entry.request.validate_for_parent(parent)?;
                conflict(request.expected_revision == parent.revision)?;
                conflict(self.import_verified(parent))?;
                let legacy = record
                    .worker_record
                    .as_ref()
                    .ok_or(ControlFailure::InvalidRequest)?;
                let mutation = WorkerMutationV1 {
                    version: 1,
                    operation_id: request.operation_id.clone(),
                    worker_id: request.worker_id.clone(),
                    attempt_id: request.attempt_id.clone(),
                    expected_revision: request.expected_revision,
                    transition: WorkerTransitionV1::StartClaim {
                        emitter_id: request.emitter_id.clone(),
                    },
                };
                invalid(
                    legacy.action == "start_claim"
                        && legacy.operation_id == request.operation_id
                        && legacy.input_hash
                            == content_hash("ai-guard:worker-mutation:v1", &mutation)?,
                )?;
            }
            Input::Bind(request) => {
                invalid(record.worker_record.is_none())?;
                let parent = self.parent(workers, &request.worker_id)?;
                conflict(
                    parent.attempt.attempt_id == request.attempt_id
                        && parent.revision == request.expected_revision
                        && parent.state == WorkerStateV1::Created
                        && parent.quota_held
                        && parent.attempt.start_operation_id.is_none()
                        && parent.attempt.created_inspection.is_some()
                        && parent.launch.security_preset.requires_no_engine_logs(),
                )?;
                self.binding(
                    parent,
                    &request.intent_hash,
                    record.input.clone(),
                    stored.seq,
                )?;
            }
            Input::Arm(request) => {
                invalid(record.worker_record.is_none())?;
                let parent = self.parent(workers, &request.worker_id)?;
                self.bound(parent, &request.intent_hash)?;
                request.validate_for_parent(parent)?;
                self.permission(db, parent, request, stored.seq)?;
                conflict(
                    !self.arms.contains_key(&request.operation_id)
                        && !self
                            .arms
                            .values()
                            .any(|a| a.request.subject_key().ok() == request.subject_key().ok()),
                )?;
                let entry = ArmEntry {
                    request: request.as_ref().clone(),
                    receipt: ArmReceiptV1 {
                        version: 1,
                        arm_id: request.operation_id.clone(),
                        worker_id: request.worker_id.clone(),
                        attempt_id: request.attempt_id.clone(),
                        input_hash: request.input_hash()?,
                        control_namespace_uuid: self.namespace.clone(),
                        audit_seq: stored.seq,
                        audit_hash: stored.hash.clone(),
                    },
                };
                self.expected
                    .get_mut("execution_arms")
                    .context("missing table")?
                    .insert(
                        request.operation_id.clone(),
                        vec![
                            sql_text(&request.operation_id),
                            sql_text(&request.worker_id),
                            sql_text(&request.attempt_id),
                            sql_text(request.subject_key()?),
                            sql_text(request.input_hash()?),
                            sql_text(serde_json::to_string(request)?),
                            rusqlite::types::Value::Integer(stored.seq),
                        ],
                    );
                self.arms.insert(request.operation_id.clone(), entry);
            }
            Input::HelperClaim(request) => {
                invalid(record.worker_record.is_none())?;
                let entry = self.checked_arm(&request.arm_receipt)?;
                let parent = self.parent(workers, &entry.request.worker_id)?;
                self.bound(parent, &entry.request.intent_hash)?;
                request.validate_for_arm(&entry.request)?;
                entry.request.validate_for_parent(parent)?;
                self.permission(db, parent, &entry.request, stored.seq)?;
                conflict(!self.claims.contains_key(&request.helper_id))?;
                self.expected
                    .get_mut("helper_claims")
                    .context("missing table")?
                    .insert(
                        request.helper_id.clone(),
                        vec![
                            sql_text(&request.helper_id),
                            sql_text(&request.arm_receipt.arm_id),
                            sql_text(&request.operation_id),
                            sql_text(&request.emitter_id),
                            sql_text(serde_json::to_string(request)?),
                            rusqlite::types::Value::Integer(stored.seq),
                        ],
                    );
                self.claims
                    .insert(request.helper_id.clone(), request.as_ref().clone());
            }
            Input::HelperObservation(request) => {
                invalid(record.worker_record.is_none())?;
                let claim = self
                    .claims
                    .get(&request.helper_id)
                    .ok_or(ControlFailure::Conflict)?;
                invalid(
                    serde_json::to_value(&claim.arm_receipt)?
                        == serde_json::to_value(&request.arm_receipt)?,
                )?;
                let entry = self.checked_arm(&request.arm_receipt)?;
                invalid(request.observation.inspection == entry.request.inspection)?;
                if let Some(previous) = self.terminals.get(&request.helper_id) {
                    let mut expected = previous.observation.clone();
                    expected.observed_at = request.observation.observed_at.clone();
                    conflict(
                        expected == request.observation
                            && (previous.verification.is_none()
                                || serde_json::to_value(&previous.verification)?
                                    == serde_json::to_value(&request.verification)?),
                    )?;
                }
                if let Some(report) = &request.verification {
                    let ArmSubjectV1::Helper {
                        purpose: HelperPurposeV1::ImportVerification,
                        permission,
                        ..
                    } = &entry.request.subject
                    else {
                        return Err(ControlFailure::InvalidRequest.into());
                    };
                    let HelperPermissionV1::ImportVerification {
                        import_helper_id,
                        manifest_digest,
                        ..
                    } = permission.as_ref()
                    else {
                        return Err(ControlFailure::InvalidRequest.into());
                    };
                    invalid(
                        &report.import_helper_id == import_helper_id
                            && &report.expected_manifest_digest == manifest_digest
                            && terminal_helper(&request.observation),
                    )?;
                }
                if terminal_helper(&request.observation) {
                    self.terminals
                        .insert(request.helper_id.clone(), request.as_ref().clone());
                }
                self.expected
                    .get_mut("helper_observations")
                    .context("missing table")?
                    .insert(
                        request.operation_id.clone(),
                        vec![
                            sql_text(&request.operation_id),
                            sql_text(&request.helper_id),
                            sql_text(serde_json::to_string(request)?),
                            rusqlite::types::Value::Integer(stored.seq),
                        ],
                    );
                self.observations
                    .insert(request.operation_id.clone(), request.as_ref().clone());
            }
            Input::HelperCleanup(request) => {
                invalid(record.worker_record.is_none())?;
                invalid(request.removed || request.error.is_some())?;
                let claim = self
                    .claims
                    .get(&request.helper_id)
                    .ok_or(ControlFailure::Conflict)?;
                invalid(
                    serde_json::to_value(&claim.arm_receipt)?
                        == serde_json::to_value(&request.arm_receipt)?,
                )?;
                conflict(self.terminals.contains_key(&request.helper_id))?;
                if let Some(old) = self.cleanups.get(&request.helper_id) {
                    conflict(!old.removed)?;
                }
                self.cleanups
                    .insert(request.helper_id.clone(), request.as_ref().clone());
            }
            Input::Export(request) => {
                invalid(record.worker_record.is_none())?;
                let parent = self.parent(workers, &request.worker_id)?;
                conflict(
                    parent.attempt.attempt_id == request.attempt_id
                        && terminal_parent(parent)
                        && self.bindings.contains_key(&parent.worker_id)
                        && !self
                            .exports
                            .values()
                            .any(|r| r.destination_ref == request.destination_ref),
                )?;
                self.exports
                    .insert(request.operation_id.clone(), request.as_ref().clone());
            }
        }
        Ok((
            record.worker_record.clone(),
            mutation_row_for(stored, &record)?,
        ))
    }
    pub(super) fn finish(self, db: &Connection) -> anyhow::Result<()> {
        invalid(self.genesis_seq.is_none() || self.seen_genesis)?;
        if self.genesis_seq.is_none() {
            return Ok(());
        }
        for (table, query) in QUERIES {
            let mut statement = db.prepare(query)?;
            let count = statement.column_count();
            let actual = statement
                .query_map([], |r| {
                    (0..count)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let expected = self
                .expected
                .get(table)
                .context("missing table")?
                .values()
                .cloned()
                .collect::<Vec<_>>();
            anyhow::ensure!(
                actual == expected,
                "supervision projection differs from audit: {table}"
            );
        }
        Ok(())
    }
}

struct Persisted {
    stored: StoredEvent,
    record: Record,
    replayed: bool,
    namespace: String,
}
impl Store {
    pub(super) fn initialize_supervision(&self, tx: &Transaction<'_>) -> anyhow::Result<()> {
        for (_, _, sql) in SCHEMA {
            tx.execute_batch(sql)?;
        }
        let (seq, hash): (i64, String) = tx.query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut event = Event::new("control.supervision.genesis");
        event.data = serde_json::to_value(Genesis {
            version: 3,
            namespace_uuid: namespace(tx)?,
            previous_seq: seq,
            previous_hash: hash,
        })?;
        let (prepared, payload) = self.prepare_event(event.clone())?;
        invalid(prepared.data == event.data)?;
        let stored = self.append_in_transaction(tx, prepared, payload)?;
        execute_row(
            tx,
            "INSERT INTO supervision_meta(singleton,protocol,genesis_seq,genesis_hash) VALUES(1,?1,?2,?3)",
            vec![
                sql_text("broker_supervision_v1"),
                rusqlite::types::Value::Integer(stored.seq),
                sql_text(stored.hash),
            ],
        )
    }
    pub fn supervision_state(&self) -> anyhow::Result<SupervisionStateV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let schema_version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let state = SupervisionStateV1 {
            version: 1,
            schema_version,
            control_namespace_uuid: if schema_version >= 2 {
                Some(namespace(&tx)?)
            } else {
                None
            },
            protocols: if [3, 4, 5, 6, 7, 8, 9, 10, 11].contains(&schema_version) {
                vec![SupervisionProtocolV1::BrokerSupervisionV1]
            } else {
                vec![]
            },
            active: [3, 4, 5, 6, 7, 8, 9, 10, 11].contains(&schema_version) && tx.lease.active,
        };
        state.validate()?;
        self.commit_checked(tx)?;
        Ok(state)
    }
    fn persist_supervision_record(
        &self,
        tx: &Transaction<'_>,
        record: &Record,
    ) -> anyhow::Result<StoredEvent> {
        let mut overlay = Replay::current(tx)?;
        let mut workers = BTreeMap::new();
        let mut stmt = tx.prepare("SELECT snapshot_json FROM workers ORDER BY worker_id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let worker: WorkerV1 = serde_json::from_str(&row?)?;
            workers.insert(worker.worker_id.clone(), worker);
        }
        drop(stmt);
        let mut event = Event::new(record.input.kind());
        event.data = serde_json::to_value(record)?;
        if let Some(id) = record.input.worker_id(record)
            && let Some(worker) = workers.get(id).or_else(|| {
                record
                    .worker_record
                    .as_ref()
                    .and_then(|r| r.worker.as_ref())
            })
        {
            event.session_id = Some(worker.launch.session_id.clone());
        }
        let (prepared, payload) = self.prepare_event(event)?;
        let mut expected = serde_json::to_value(record)?;
        if let Some(policy) = expected.pointer_mut("/worker_record/admission_policy") {
            *policy = self.redactor.value(policy);
        }
        invalid(prepared.data == expected)?;
        let stored = self.append_in_transaction(tx, prepared, payload)?;
        let (_, mutation_row) = overlay.apply(tx, &stored, &workers)?;
        if let Some(legacy) = &record.worker_record {
            self.write_control_projection(
                tx,
                legacy,
                matches!(&record.input, Input::Admission(_)),
                &stored,
                mutation_row,
            )?;
        } else {
            execute_row(
                tx,
                "INSERT INTO worker_mutations(operation_id,worker_id,action,input_hash,event_seq) VALUES(?1,?2,?3,?4,?5)",
                mutation_row,
            )?;
        }
        for (table, rows) in &overlay.expected {
            let sql = match *table {
                "worker_supervision" => {
                    "INSERT INTO worker_supervision(worker_id,intent_hash,payload_json,event_seq) VALUES(?1,?2,?3,?4)"
                }
                "execution_arms" => {
                    "INSERT INTO execution_arms(arm_id,worker_id,attempt_id,subject_key,input_hash,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7)"
                }
                "helper_claims" => {
                    "INSERT INTO helper_claims(helper_id,arm_id,operation_id,emitter_id,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6)"
                }
                "helper_observations" => {
                    "INSERT INTO helper_observations(operation_id,helper_id,payload_json,event_seq) VALUES(?1,?2,?3,?4)"
                }
                "supervision_meta" => {
                    invalid(rows.is_empty())?;
                    continue;
                }
                _ => return Err(ControlFailure::InvalidRequest.into()),
            };
            for row in rows.values() {
                execute_row(tx, sql, row.clone())?;
            }
        }
        if let Err(error) = validate(tx) {
            self.unhealthy.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(stored)
    }
    fn persist_supervision_input(
        &self,
        input: Input,
        build: impl FnOnce(&Transaction<'_>) -> anyhow::Result<Option<ControlRecord>>,
    ) -> anyhow::Result<Persisted> {
        let input_hash = input.hash()?;
        let value = serde_json::to_value(&input)?;
        invalid(
            self.redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536,
        )?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_v3(&tx)?;
        let namespace = namespace(&tx)?;
        if let Some((stored, record)) = load_record(&tx, input.operation_id())? {
            conflict(
                record.input.hash()? == input_hash && record.input.action() == input.action(),
            )?;
            self.commit_checked(tx)?;
            return Ok(Persisted {
                stored,
                record,
                replayed: true,
                namespace,
            });
        }
        if matches!(&input, Input::Bind(_)) {
            return Err(ControlFailure::SupervisionInactive.into());
        }
        if !tx.lease.active
            && !matches!(
                &input,
                Input::HelperObservation(_) | Input::HelperCleanup(_)
            )
        {
            return Err(ControlFailure::SupervisionInactive.into());
        }
        let worker_record = build(&tx)?;
        let record = Record {
            version: 2,
            input,
            worker_record,
        };
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.commit_checked(tx)?;
        Ok(Persisted {
            stored,
            record,
            replayed: false,
            namespace,
        })
    }
    pub fn admit_supervised(
        &self,
        request: &SupervisedAdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<AdmissionResultV1> {
        request.validate()?;
        quota.validate()?;
        let saved =
            self.persist_supervision_input(Input::Admission(Box::new(request.clone())), |tx| {
                Ok(Some(self.build_admission_record(
                    tx,
                    &request.admission,
                    policy,
                    fresh_decision,
                    jev,
                    quota,
                )?))
            })?;
        let legacy = saved
            .record
            .worker_record
            .as_ref()
            .ok_or(ControlFailure::InvalidRequest)?;
        let mut result = admission_result(&saved.stored, legacy);
        result.receipt = receipt_for(&saved.stored, &saved.record)?;
        result.replayed = saved.replayed;
        Ok(result)
    }
    pub fn bind_legacy_intent(
        &self,
        request: &BindLegacyIntentV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved =
            self.persist_supervision_input(Input::Bind(Box::new(request.clone())), |_| Ok(None))?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn arm_execution(&self, request: &ArmV1) -> anyhow::Result<ArmResultV1> {
        let saved =
            self.persist_supervision_input(Input::Arm(Box::new(request.clone())), |_| Ok(None))?;
        let Input::Arm(arm) = &saved.record.input else {
            return Err(ControlFailure::InvalidRequest.into());
        };
        Ok(ArmResultV1 {
            replayed: saved.replayed,
            arm: arm.as_ref().clone(),
            receipt: ArmReceiptV1 {
                version: 1,
                arm_id: arm.operation_id.clone(),
                worker_id: arm.worker_id.clone(),
                attempt_id: arm.attempt_id.clone(),
                input_hash: arm.input_hash()?,
                control_namespace_uuid: saved.namespace,
                audit_seq: saved.stored.seq,
                audit_hash: saved.stored.hash,
            },
        })
    }
    pub fn execution_arm(&self, id: &str) -> anyhow::Result<Option<ArmResultV1>> {
        uuid(id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_v3(&tx)?;
        let result = arm_entry(&tx, id)?.map(|entry| ArmResultV1 {
            replayed: true,
            arm: entry.request,
            receipt: entry.receipt,
        });
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn claim_armed_start(
        &self,
        request: &ArmedStartClaimV1,
    ) -> anyhow::Result<MutationResultV1> {
        let saved =
            self.persist_supervision_input(Input::WorkerClaim(Box::new(request.clone())), |tx| {
                let mut worker =
                    load_worker(tx, &request.worker_id)?.ok_or(ControlFailure::NotFound)?;
                conflict(
                    worker.attempt.attempt_id == request.attempt_id
                        && worker.revision == request.expected_revision,
                )?;
                let mutation = WorkerMutationV1 {
                    version: 1,
                    operation_id: request.operation_id.clone(),
                    worker_id: request.worker_id.clone(),
                    attempt_id: request.attempt_id.clone(),
                    expected_revision: request.expected_revision,
                    transition: WorkerTransitionV1::StartClaim {
                        emitter_id: request.emitter_id.clone(),
                    },
                };
                let (observation, allocation) = apply_worker_transition(&mut worker, &mutation)?;
                Ok(Some(ControlRecord {
                    version: 1,
                    operation_id: request.operation_id.clone(),
                    action: "start_claim".into(),
                    input_hash: content_hash("ai-guard:worker-mutation:v1", &mutation)?,
                    previous_revision: Some(request.expected_revision),
                    snapshot_hash: Some(content_hash("ai-guard:worker-snapshot:v1", &worker)?),
                    worker: Some(worker),
                    observation,
                    allocation,
                    refusal: None,
                    admission_policy: None,
                }))
            })?;
        let worker = saved
            .record
            .worker_record
            .as_ref()
            .and_then(|r| r.worker.clone())
            .ok_or(ControlFailure::InvalidRequest)?;
        Ok(MutationResultV1 {
            worker,
            receipt: receipt_for(&saved.stored, &saved.record)?,
            disposition: Some(if saved.replayed {
                ClaimDispositionV1::AlreadyConsumed
            } else {
                ClaimDispositionV1::EmitOnce
            }),
        })
    }
    pub fn claim_helper_start(
        &self,
        request: &HelperStartClaimV1,
    ) -> anyhow::Result<HelperClaimResultV1> {
        let saved = self
            .persist_supervision_input(Input::HelperClaim(Box::new(request.clone())), |_| {
                Ok(None)
            })?;
        Ok(HelperClaimResultV1 {
            receipt: receipt_for(&saved.stored, &saved.record)?,
            disposition: if saved.replayed {
                ClaimDispositionV1::AlreadyConsumed
            } else {
                ClaimDispositionV1::EmitOnce
            },
        })
    }
    pub fn observe_helper(
        &self,
        request: &HelperObservationV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved = self.persist_supervision_input(
            Input::HelperObservation(Box::new(request.clone())),
            |_| Ok(None),
        )?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn record_helper_cleanup(
        &self,
        request: &HelperCleanupV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved = self
            .persist_supervision_input(Input::HelperCleanup(Box::new(request.clone())), |_| {
                Ok(None)
            })?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn request_export(&self, request: &ExportRequestV1) -> anyhow::Result<MutationReceiptV1> {
        let saved =
            self.persist_supervision_input(Input::Export(Box::new(request.clone())), |_| Ok(None))?;
        receipt_for(&saved.stored, &saved.record)
    }
}

pub(super) fn can_create(db: &Connection, worker: &WorkerV1) -> anyhow::Result<()> {
    let state = Replay::current(db)?;
    conflict(state.bindings.contains_key(&worker.worker_id) && state.import_verified(worker))
}
