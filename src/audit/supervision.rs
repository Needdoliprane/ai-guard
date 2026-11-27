//! Audited supervision on explicitly admitted Store transactions. No automatic migration.
use super::*;
use crate::audit::{
    legacy_session_prepare::{
        LEGACY_NAMED_ARM_KIND, LEGACY_NAMED_IMPORT_CLAIM_KIND,
        LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND, LegacyNamedArmV1, LegacyNamedCancelV1,
        LegacyNamedImportClaimV1, legacy_named_import_claim_predecessor,
    },
    supervision_admission::{self as admission_codec, Binding, receipt_for},
    supervision_genesis,
};
use crate::control::session_control::LegacyNamedImportObservationRequestV1;
use std::collections::BTreeSet;

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
pub(crate) enum Input {
    Admission(Box<SupervisedAdmissionV1>),
    Bind(Box<BindLegacyIntentV1>),
    Arm(Box<ArmV1>),
    LegacyNamedArm(Box<LegacyNamedArmV1>),
    LegacyNamedImportVerificationArm(Box<LegacyNamedArmV1>),
    LegacyNamedCancel(Box<LegacyNamedCancelV1>),
    WorkerClaim(Box<ArmedStartClaimV1>),
    HelperClaim(Box<HelperStartClaimV1>),
    LegacyNamedImportClaim(Box<LegacyNamedImportClaimV1>),
    HelperObservation(Box<HelperObservationV1>),
    LegacyNamedImportObservation(Box<LegacyNamedImportObservationV1>),
    HelperCleanup(Box<HelperCleanupV1>),
    Export(Box<ExportRequestV1>),
}

const LEGACY_NAMED_IMPORT_OBSERVATION_KIND: &str =
    "control.execution.helper_observation.legacy_named";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportObservationV1 {
    pub(in crate::audit) version: u32,
    pub(in crate::audit) request: LegacyNamedImportObservationRequestV1,
    pub(in crate::audit) session_id: String,
    pub(in crate::audit) authorized_at: chrono::DateTime<chrono::Utc>,
}

impl LegacyNamedImportObservationV1 {
    fn input_hash(&self) -> anyhow::Result<String> {
        invalid(
            self.version == 1
                && super::workload::canonical_control_time(self.authorized_at)?
                    == self.authorized_at,
        )?;
        uuid(&self.session_id)?;
        self.request.input_hash()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(in crate::audit) version: u32,
    pub(in crate::audit) input: Input,
    pub(in crate::audit) worker_record: Option<ControlRecord>,
}
pub(super) fn worker_snapshot(stored: &StoredEvent) -> anyhow::Result<Option<WorkerV1>> {
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    invalid(record.version == 2 && stored.event.kind == record.input.kind())?;
    Ok(record.worker_record.and_then(|record| record.worker))
}
#[derive(Clone)]
struct ArmEntry {
    request: ArmV1,
    receipt: ArmReceiptV1,
    legacy_named: bool,
}

impl Input {
    pub(crate) fn operation_id(&self) -> &str {
        match self {
            Self::Admission(r) => &r.admission.operation_id,
            Self::Bind(r) => &r.operation_id,
            Self::Arm(r) => &r.operation_id,
            Self::LegacyNamedArm(r) | Self::LegacyNamedImportVerificationArm(r) => {
                &r.arm.operation_id
            }
            Self::LegacyNamedCancel(r) => &r.request.mutation.operation_id,
            Self::WorkerClaim(r) => &r.operation_id,
            Self::HelperClaim(r) => &r.operation_id,
            Self::LegacyNamedImportClaim(r) => &r.request.claim.operation_id,
            Self::HelperObservation(r) => &r.operation_id,
            Self::LegacyNamedImportObservation(r) => &r.request.observation.operation_id,
            Self::HelperCleanup(r) => &r.operation_id,
            Self::Export(r) => &r.operation_id,
        }
    }
    pub(crate) fn action(&self) -> &'static str {
        match self {
            Self::Admission(_) => admission_codec::ACTION,
            Self::Bind(_) => "bind_intent",
            Self::Arm(_) | Self::LegacyNamedImportVerificationArm(_) => "arm",
            // The existing snapshot index includes this state transition.
            Self::LegacyNamedArm(_) => "prepare",
            Self::LegacyNamedCancel(_) => "cancel_reserved",
            Self::WorkerClaim(_) => "armed_start_claim",
            Self::HelperClaim(_) | Self::LegacyNamedImportClaim(_) => "helper_start_claim",
            Self::HelperObservation(_) | Self::LegacyNamedImportObservation(_) => "helper_observe",
            Self::HelperCleanup(_) => "helper_cleanup",
            Self::Export(_) => "export_request",
        }
    }
    pub(in crate::audit) fn kind(&self) -> &'static str {
        match self {
            Self::Arm(_) => "control.execution.arm",
            Self::LegacyNamedArm(_) => LEGACY_NAMED_ARM_KIND,
            Self::LegacyNamedImportVerificationArm(_) => LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND,
            Self::LegacyNamedImportClaim(_) => LEGACY_NAMED_IMPORT_CLAIM_KIND,
            Self::LegacyNamedImportObservation(_) => LEGACY_NAMED_IMPORT_OBSERVATION_KIND,
            _ => admission_codec::KIND,
        }
    }
    pub(crate) fn hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Admission(r) => r.input_hash(),
            Self::Bind(r) => r.input_hash(),
            Self::Arm(r) => r.input_hash(),
            Self::LegacyNamedArm(r) => r.input_hash(),
            Self::LegacyNamedImportVerificationArm(r) => r.verification_input_hash(),
            Self::LegacyNamedCancel(r) => r.input_hash(),
            Self::WorkerClaim(r) => r.input_hash(),
            Self::HelperClaim(r) => r.input_hash(),
            Self::LegacyNamedImportClaim(r) => r.input_hash(),
            Self::HelperObservation(r) => r.input_hash(),
            Self::LegacyNamedImportObservation(r) => r.input_hash(),
            Self::HelperCleanup(r) => r.input_hash(),
            Self::Export(r) => r.input_hash(),
        }
    }
    pub(in crate::audit) fn worker_id<'a>(&'a self, record: &'a Record) -> Option<&'a str> {
        match self {
            Self::Admission(_) => record
                .worker_record
                .as_ref()
                .and_then(|r| r.worker.as_ref())
                .map(|w| w.worker_id.as_str()),
            Self::Bind(r) => Some(&r.worker_id),
            Self::Arm(r) => Some(&r.worker_id),
            Self::LegacyNamedArm(r) | Self::LegacyNamedImportVerificationArm(r) => {
                Some(&r.arm.worker_id)
            }
            Self::LegacyNamedCancel(r) => Some(&r.request.mutation.worker_id),
            Self::WorkerClaim(r) => Some(&r.worker_id),
            Self::HelperClaim(r) => Some(&r.arm_receipt.worker_id),
            Self::LegacyNamedImportClaim(r) => Some(&r.request.claim.arm_receipt.worker_id),
            Self::HelperObservation(r) => Some(&r.arm_receipt.worker_id),
            Self::LegacyNamedImportObservation(r) => {
                Some(&r.request.observation.arm_receipt.worker_id)
            }
            Self::HelperCleanup(r) => Some(&r.arm_receipt.worker_id),
            Self::Export(r) => Some(&r.worker_id),
        }
    }
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
pub(in crate::audit) fn load_record(
    db: &Connection,
    id: &str,
) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let stored=db.query_row("SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.operation_id=?1",[id],row_event).optional()?;
    stored
        .map(|stored: StoredEvent| {
            conflict(matches!(
                stored.event.kind.as_str(),
                "control.execution.arm"
                    | LEGACY_NAMED_ARM_KIND
                    | LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND
                    | LEGACY_NAMED_IMPORT_CLAIM_KIND
                    | LEGACY_NAMED_IMPORT_OBSERVATION_KIND
                    | "control.supervision.transition"
            ))?;
            let record: Record = serde_json::from_value(stored.event.data.clone())?;
            invalid(record.version == 2 && record.input.kind() == stored.event.kind)?;
            Ok((stored, record))
        })
        .transpose()
}
pub(in crate::audit) fn operation_receipt(
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
fn require_v3(
    db: &CheckedTransaction<'_>,
    access: Option<&HumanSourceAccess<'_>>,
) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        Ok(())
    } else if matches!(version, 14 | 15) && access.is_some() {
        db.require_business_access()
    } else {
        Err(ControlFailure::MigrationRequired.into())
    }
}
pub(in crate::audit) fn namespace(db: &Connection) -> anyhow::Result<String> {
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
    let (request, legacy_named) = match record.input {
        Input::Arm(request) => (*request, false),
        Input::LegacyNamedArm(request) | Input::LegacyNamedImportVerificationArm(request) => {
            (request.arm, true)
        }
        _ => return Err(ControlFailure::Conflict.into()),
    };
    let receipt = arm_receipt(&request, &namespace(db)?, &stored)?;
    Ok(Some(ArmEntry {
        request,
        receipt,
        legacy_named,
    }))
}

pub(in crate::audit) fn arm_receipt(
    request: &ArmV1,
    namespace: &str,
    stored: &StoredEvent,
) -> anyhow::Result<ArmReceiptV1> {
    Ok(ArmReceiptV1 {
        version: 1,
        arm_id: request.operation_id.clone(),
        worker_id: request.worker_id.clone(),
        attempt_id: request.attempt_id.clone(),
        input_hash: request.input_hash()?,
        control_namespace_uuid: namespace.into(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    })
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
    armed_workers: BTreeSet<String>,
    claims: BTreeMap<String, HelperStartClaimV1>,
    observations: BTreeMap<String, HelperObservationV1>,
    generic_terminals: BTreeMap<String, HelperObservationV1>,
    named_import_terminals: BTreeMap<String, HelperObservationV1>,
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
            state.armed_workers.insert(entry.request.worker_id.clone());
            state.arms.insert(id, entry);
        }
        let mut stmt = db.prepare("SELECT payload_json FROM helper_claims ORDER BY helper_id")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let claim: HelperStartClaimV1 = serde_json::from_str(&row?)?;
            state.claims.insert(claim.helper_id.clone(), claim);
        }
        let mut stmt = db.prepare(
            "SELECT o.payload_json,e.kind FROM helper_observations o
             JOIN events e ON e.seq=o.event_seq ORDER BY o.event_seq",
        )?;
        for row in stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (payload, kind) = row?;
            let observation: HelperObservationV1 = serde_json::from_str(&payload)?;
            let terminals = state.observation_terminals(&kind)?;
            if terminal_helper(&observation.observation) {
                terminals.insert(observation.helper_id.clone(), observation.clone());
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
        let (genesis_seq, genesis_hash) =
            if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version) {
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
            armed_workers: BTreeSet::new(),
            claims: BTreeMap::new(),
            observations: BTreeMap::new(),
            generic_terminals: BTreeMap::new(),
            named_import_terminals: BTreeMap::new(),
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
        supervision_genesis::validate(stored, &self.namespace)?;
        invalid(
            !self.seen_genesis
                && Some(stored.seq) == self.genesis_seq
                && Some(&stored.hash) == self.genesis_hash.as_ref(),
        )?;
        self.seen_genesis = true;
        self.expected
            .get_mut("supervision_meta")
            .context("missing table")?
            .insert(
                "1".into(),
                vec![
                    rusqlite::types::Value::Integer(1),
                    sql_text(supervision_genesis::PROTOCOL),
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
    fn observation_terminals(
        &mut self,
        event_kind: &str,
    ) -> anyhow::Result<&mut BTreeMap<String, HelperObservationV1>> {
        match event_kind {
            admission_codec::KIND => Ok(&mut self.generic_terminals),
            LEGACY_NAMED_IMPORT_OBSERVATION_KIND => Ok(&mut self.named_import_terminals),
            _ => Err(ControlFailure::InvalidRequest.into()),
        }
    }
    fn terminal_for_arm(&self, entry: &ArmEntry, helper_id: &str) -> Option<&HelperObservationV1> {
        if entry.legacy_named {
            self.named_import_terminals.get(helper_id)
        } else {
            self.generic_terminals.get(helper_id)
        }
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
            let Some(observation) = self.terminal_for_arm(entry, helper_id) else {
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
    fn checked_manual_arm(&self, given: &ArmReceiptV1) -> anyhow::Result<&ArmEntry> {
        let entry = self.checked_arm(given)?;
        conflict(!entry.legacy_named)?;
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
                    .terminal_for_arm(entry, import_helper_id)
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
    fn record_arm(
        &mut self,
        db: &Connection,
        stored: &StoredEvent,
        parent: &WorkerV1,
        request: &ArmV1,
        legacy_named: bool,
    ) -> anyhow::Result<()> {
        self.bound(parent, &request.intent_hash)?;
        request.validate_for_parent(parent)?;
        self.permission(db, parent, request, stored.seq)?;
        let subject_key = request.subject_key()?;
        conflict(
            !self.arms.contains_key(&request.operation_id)
                && !self.arms.values().any(|entry| {
                    entry
                        .request
                        .subject_key()
                        .is_ok_and(|key| key == subject_key)
                }),
        )?;
        let receipt = arm_receipt(request, &self.namespace, stored)?;
        self.expected
            .get_mut("execution_arms")
            .context("missing table")?
            .insert(
                request.operation_id.clone(),
                vec![
                    sql_text(&request.operation_id),
                    sql_text(&request.worker_id),
                    sql_text(&request.attempt_id),
                    sql_text(subject_key),
                    sql_text(&receipt.input_hash),
                    sql_text(serde_json::to_string(request)?),
                    rusqlite::types::Value::Integer(stored.seq),
                ],
            );
        self.armed_workers.insert(request.worker_id.clone());
        self.arms.insert(
            request.operation_id.clone(),
            ArmEntry {
                request: request.clone(),
                receipt,
                legacy_named,
            },
        );
        Ok(())
    }
    fn record_helper_claim(
        &mut self,
        db: &Connection,
        stored: &StoredEvent,
        workers: &BTreeMap<String, WorkerV1>,
        request: &HelperStartClaimV1,
        named: Option<&LegacyNamedImportClaimV1>,
    ) -> anyhow::Result<()> {
        let entry = self.checked_arm(&request.arm_receipt)?;
        conflict(entry.legacy_named == named.is_some())?;
        let parent = self.parent(workers, &entry.request.worker_id)?;
        if let Some(named) = named {
            legacy_named_import_claim_predecessor(db, stored, named, parent)?;
        }
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
            .insert(request.helper_id.clone(), request.clone());
        Ok(())
    }

    fn record_helper_observation(
        &mut self,
        stored: &StoredEvent,
        request: &HelperObservationV1,
    ) -> anyhow::Result<()> {
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
        let terminals = self.observation_terminals(&stored.event.kind)?;
        if let Some(previous) = terminals.get(&request.helper_id) {
            let mut expected = previous.observation.clone();
            expected.observed_at = request.observation.observed_at.clone();
            conflict(
                expected == request.observation
                    && (previous.verification.is_none()
                        || serde_json::to_value(&previous.verification)?
                            == serde_json::to_value(&request.verification)?),
            )?;
        }
        if terminal_helper(&request.observation) {
            terminals.insert(request.helper_id.clone(), request.clone());
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
            .insert(request.operation_id.clone(), request.clone());
        Ok(())
    }

    pub(super) fn apply(
        &mut self,
        db: &Connection,
        stored: &StoredEvent,
        workers: &BTreeMap<String, WorkerV1>,
    ) -> anyhow::Result<(Option<ControlRecord>, Vec<rusqlite::types::Value>)> {
        let record = admission_codec::decode(stored)?;
        invalid(self.seen_genesis && self.genesis_seq.is_some_and(|seq| stored.seq > seq))?;
        record.input.hash()?;
        match &record.input {
            Input::Admission(_) => {
                let (request, legacy) = record.admission()?;
                if let Some(worker) = &legacy.worker {
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
                let entry = self.checked_manual_arm(&request.arm_receipt)?;
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
                legacy_session_prepare::protect_manual_at(
                    db,
                    &parent.admission_operation_id,
                    &parent.launch.session_id,
                    stored.seq,
                )?;
                self.record_arm(db, stored, parent, request, false)?;
            }
            Input::LegacyNamedArm(request) => {
                let parent = self.parent(workers, &request.arm.worker_id)?;
                legacy_session_prepare::legacy_named_arm_predecessor(
                    db, request, parent, stored.seq,
                )?;
                let prepared = record
                    .worker_record
                    .as_ref()
                    .ok_or(ControlFailure::Conflict)?;
                conflict(
                    prepared.action == "prepare"
                        && prepared.operation_id == request.arm.operation_id
                        && stored.event.session_id.as_deref() == Some(request.session_id.as_str())
                        && stored.event.coverage == "observed"
                        && request.authorized_at
                            <= crate::broker_registry::parse_time(&stored.received_at)?,
                )?;
                worker_transition::replay_record(parent, prepared)?;
                let worker = prepared.worker.as_ref().ok_or(ControlFailure::Conflict)?;
                self.record_arm(db, stored, worker, &request.arm, true)?;
            }
            Input::LegacyNamedImportVerificationArm(request) => {
                invalid(record.worker_record.is_none())?;
                let parent = self.parent(workers, &request.arm.worker_id)?;
                Store::validate_named_import_verification_arm_predecessor(
                    db, stored, request, parent,
                )?;
                self.record_arm(db, stored, parent, &request.arm, true)?;
            }
            Input::LegacyNamedCancel(cancel) => {
                let parent = self.parent(workers, &cancel.request.mutation.worker_id)?;
                // Every supervised claim resolves an earlier arm. The immutable
                // original Reserved snapshot also excludes a legacy start.
                conflict(!self.armed_workers.contains(&parent.worker_id))?;
                let transition = record
                    .worker_record
                    .as_ref()
                    .ok_or(ControlFailure::Conflict)?;
                legacy_session_prepare::validate_legacy_named_cancel(
                    db, stored, cancel, parent, transition,
                )?;
            }
            Input::HelperClaim(request) => {
                invalid(record.worker_record.is_none())?;
                self.record_helper_claim(db, stored, workers, request, None)?;
            }
            Input::LegacyNamedImportClaim(named) => {
                invalid(record.worker_record.is_none())?;
                self.record_helper_claim(db, stored, workers, &named.request.claim, Some(named))?;
            }
            Input::HelperObservation(request) => {
                invalid(record.worker_record.is_none())?;
                // Historical observations retain their generic provenance.
                self.record_helper_observation(stored, request)?;
            }
            Input::LegacyNamedImportObservation(named) => {
                invalid(record.worker_record.is_none())?;
                Store::validate_named_import_observation_predecessor(db, stored, named)?;
                self.record_helper_observation(stored, &named.request.observation)?;
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
                conflict(self.generic_terminals.contains_key(&request.helper_id))?;
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
        let event = supervision_genesis::event(&namespace(tx)?, seq, &hash)?;
        let (prepared, payload) = self.prepare_event(event.clone())?;
        invalid(prepared.data == event.data)?;
        let stored = self.append_in_transaction(tx, prepared, payload)?;
        execute_row(
            tx,
            "INSERT INTO supervision_meta(singleton,protocol,genesis_seq,genesis_hash) VALUES(1,?1,?2,?3)",
            vec![
                sql_text(supervision_genesis::PROTOCOL),
                rusqlite::types::Value::Integer(stored.seq),
                sql_text(stored.hash),
            ],
        )
    }
    pub fn supervision_state(&self) -> anyhow::Result<SupervisionStateV1> {
        self.supervision_state_with_access(None)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn supervision_state_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
    ) -> anyhow::Result<SupervisionStateV1> {
        self.supervision_state_with_access(Some(access))
    }
    fn supervision_state_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
    ) -> anyhow::Result<SupervisionStateV1> {
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
        let state = self.supervision_state_checked(&tx)?;
        self.commit_checked(tx)?;
        Ok(state)
    }

    /// Shared diagnostic reader. The actual checked transaction decides
    /// whether modern Business access exists; a caller-supplied flag cannot.
    pub(in crate::audit) fn supervision_state_checked(
        &self,
        tx: &CheckedTransaction<'_>,
    ) -> anyhow::Result<SupervisionStateV1> {
        let schema_version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let supported = [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&schema_version)
            || (matches!(schema_version, 14 | 15) && tx.source_access.is_some());
        if matches!(schema_version, 14 | 15) && tx.source_access.is_some() {
            tx.require_business_access()?;
        }
        let state = SupervisionStateV1 {
            version: 1,
            schema_version,
            control_namespace_uuid: if schema_version >= 2 {
                Some(namespace(tx)?)
            } else {
                None
            },
            protocols: if supported {
                vec![SupervisionProtocolV1::BrokerSupervisionV1]
            } else {
                vec![]
            },
            active: supported && tx.lease.active,
        };
        state.validate()?;
        Ok(state)
    }
    /// Writes the supervision event and its projections in the caller's transaction.
    pub(in crate::audit) fn persist_supervision_record(
        &self,
        tx: &CheckedTransaction<'_>,
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
        let session_id = record.input.worker_id(record).and_then(|id| {
            workers
                .get(id)
                .or_else(|| {
                    record
                        .worker_record
                        .as_ref()
                        .and_then(|r| r.worker.as_ref())
                })
                .map(|worker| worker.launch.session_id.as_str())
        });
        let event = admission_codec::event(record, session_id)?;
        let (prepared, payload) = self.prepare_event(event)?;
        admission_codec::require_prepared(&prepared, record, &self.redactor)?;
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
        Ok(stored)
    }
    pub(in crate::audit) fn validate_supervision_write(
        &self,
        tx: &CheckedTransaction<'_>,
    ) -> anyhow::Result<()> {
        if let Err(error) = validate(tx) {
            self.unhealthy.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }
    fn persist_supervision_input(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        input: Input,
        build: impl FnOnce(&Transaction<'_>) -> anyhow::Result<Option<ControlRecord>>,
    ) -> anyhow::Result<Persisted> {
        let input_hash = admission_codec::validate_input(&input, &self.redactor)?;
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
        require_v3(&tx, access)?;
        let namespace = namespace(&tx)?;
        if let Input::Admission(request) = &input {
            super::legacy_session_prepare::protect_manual(
                &tx,
                &request.admission.operation_id,
                &request.admission.launch.session_id,
            )?;
        }
        if let Some((stored, record)) = load_record(&tx, input.operation_id())? {
            admission_codec::require_replay(&record, &input, &input_hash)?;
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
        let manual_arm = match &input {
            Input::WorkerClaim(request) => Some(&request.arm_receipt.arm_id),
            Input::HelperClaim(request) => Some(&request.arm_receipt.arm_id),
            Input::HelperObservation(request) => Some(&request.arm_receipt.arm_id),
            Input::HelperCleanup(request) => Some(&request.arm_receipt.arm_id),
            _ => None,
        };
        if let Some(id) = manual_arm {
            let named: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM worker_mutations m JOIN events e ON e.seq=m.event_seq
                 WHERE m.operation_id=?1 AND e.kind IN (?2,?3))",
                params![
                    id,
                    LEGACY_NAMED_ARM_KIND,
                    LEGACY_NAMED_IMPORT_VERIFICATION_ARM_KIND
                ],
                |row| row.get(0),
            )?;
            conflict(!named)?;
        }
        let worker_record = build(&tx)?;
        let record = Record::new(input, worker_record);
        let stored = self.persist_supervision_record(&tx, &record)?;
        self.validate_supervision_write(&tx)?;
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
        self.admit_supervised_with_access(None, request, policy, fresh_decision, jev, quota)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn admit_supervised_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &SupervisedAdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<AdmissionResultV1> {
        self.admit_supervised_with_access(Some(access), request, policy, fresh_decision, jev, quota)
    }
    fn admit_supervised_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &SupervisedAdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<AdmissionResultV1> {
        let input = admission_codec::admission_input(request, quota)?;
        let saved = self.persist_supervision_input(access, input, |tx| {
            Ok(Some(self.build_admission_record(
                tx,
                &request.admission,
                policy,
                fresh_decision,
                jev,
                quota,
            )?))
        })?;
        admission_codec::result(&saved.stored, &saved.record, saved.replayed)
    }
    pub fn bind_legacy_intent(
        &self,
        request: &BindLegacyIntentV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.bind_legacy_intent_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn bind_legacy_intent_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &BindLegacyIntentV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.bind_legacy_intent_with_access(Some(access), request)
    }
    fn bind_legacy_intent_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &BindLegacyIntentV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved =
            self.persist_supervision_input(access, Input::Bind(Box::new(request.clone())), |_| {
                Ok(None)
            })?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn arm_execution(&self, request: &ArmV1) -> anyhow::Result<ArmResultV1> {
        self.arm_execution_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn arm_execution_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &ArmV1,
    ) -> anyhow::Result<ArmResultV1> {
        self.arm_execution_with_access(Some(access), request)
    }
    fn arm_execution_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &ArmV1,
    ) -> anyhow::Result<ArmResultV1> {
        let saved =
            self.persist_supervision_input(access, Input::Arm(Box::new(request.clone())), |tx| {
                // Once the named claim exists it will own the corresponding arm path.
                let worker =
                    load_worker(tx, &request.worker_id)?.ok_or(ControlFailure::NotFound)?;
                super::legacy_session_prepare::protect_manual(
                    tx,
                    &worker.admission_operation_id,
                    &worker.launch.session_id,
                )?;
                Ok(None)
            })?;
        let Input::Arm(arm) = &saved.record.input else {
            return Err(ControlFailure::InvalidRequest.into());
        };
        Ok(ArmResultV1 {
            replayed: saved.replayed,
            arm: arm.as_ref().clone(),
            receipt: arm_receipt(arm, &saved.namespace, &saved.stored)?,
        })
    }
    pub fn execution_arm(&self, id: &str) -> anyhow::Result<Option<ArmResultV1>> {
        self.execution_arm_with_access(None, id)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn execution_arm_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        id: &str,
    ) -> anyhow::Result<Option<ArmResultV1>> {
        self.execution_arm_with_access(Some(access), id)
    }
    fn execution_arm_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        id: &str,
    ) -> anyhow::Result<Option<ArmResultV1>> {
        uuid(id)?;
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
        require_v3(&tx, access)?;
        let result = arm_entry(&tx, id)?
            .filter(|entry| !entry.legacy_named)
            .map(|entry| ArmResultV1 {
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
        self.claim_armed_start_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn claim_armed_start_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &ArmedStartClaimV1,
    ) -> anyhow::Result<MutationResultV1> {
        self.claim_armed_start_with_access(Some(access), request)
    }
    fn claim_armed_start_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &ArmedStartClaimV1,
    ) -> anyhow::Result<MutationResultV1> {
        let saved = self.persist_supervision_input(
            access,
            Input::WorkerClaim(Box::new(request.clone())),
            |tx| {
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
            },
        )?;
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
        self.claim_helper_start_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn claim_helper_start_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &HelperStartClaimV1,
    ) -> anyhow::Result<HelperClaimResultV1> {
        self.claim_helper_start_with_access(Some(access), request)
    }
    fn claim_helper_start_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &HelperStartClaimV1,
    ) -> anyhow::Result<HelperClaimResultV1> {
        let saved = self.persist_supervision_input(
            access,
            Input::HelperClaim(Box::new(request.clone())),
            |_| Ok(None),
        )?;
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
        self.observe_helper_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn observe_helper_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &HelperObservationV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.observe_helper_with_access(Some(access), request)
    }
    fn observe_helper_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &HelperObservationV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved = self.persist_supervision_input(
            access,
            Input::HelperObservation(Box::new(request.clone())),
            |_| Ok(None),
        )?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn record_helper_cleanup(
        &self,
        request: &HelperCleanupV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.record_helper_cleanup_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn record_helper_cleanup_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &HelperCleanupV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.record_helper_cleanup_with_access(Some(access), request)
    }
    fn record_helper_cleanup_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &HelperCleanupV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved = self.persist_supervision_input(
            access,
            Input::HelperCleanup(Box::new(request.clone())),
            |_| Ok(None),
        )?;
        receipt_for(&saved.stored, &saved.record)
    }
    pub fn request_export(&self, request: &ExportRequestV1) -> anyhow::Result<MutationReceiptV1> {
        self.request_export_with_access(None, request)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn request_export_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        request: &ExportRequestV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        self.request_export_with_access(Some(access), request)
    }
    fn request_export_with_access(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        request: &ExportRequestV1,
    ) -> anyhow::Result<MutationReceiptV1> {
        let saved = self.persist_supervision_input(
            access,
            Input::Export(Box::new(request.clone())),
            |_| Ok(None),
        )?;
        receipt_for(&saved.stored, &saved.record)
    }
}

pub(super) fn can_create(db: &Connection, worker: &WorkerV1) -> anyhow::Result<()> {
    let state = Replay::current(db)?;
    conflict(state.bindings.contains_key(&worker.worker_id) && state.import_verified(worker))
}

#[cfg(test)]
#[path = "supervision/scoped_tests.rs"]
mod scoped_tests;
