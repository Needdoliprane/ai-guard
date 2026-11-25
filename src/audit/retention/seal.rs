//! G06/R2 first slice: read-only selection of a closed supervised worker on
//! the maintenance path (arbitration of 25 November 2026, DECISIONS.md).
//!
//! The selection examines one worker and its attempt in a single validated
//! snapshot: the live hash chain verifies, then the complete control and
//! supervision replay (`control::validate`) proves every projection against
//! the audit. It returns the exact ordered proof references from the decision
//! and admission to the confirmed cleanup, anchored to the verified Store
//! head and cross-checked with the broker Full journal head of the same
//! operation. A pending operation, a missing observation or a divergent
//! projection refuses the selection without changing anything.
//!
//! Limits of this slice: it is internal and read-only. The broker head is the
//! settled journal snapshot the maintenance caller reads for the worker's
//! start operation; the durable Store-owned freeze, the serialized sealing
//! commit and the verifiable dossier publication come with the next slices.

use super::failure::{RetentionFailure, code};
use crate::{
    audit::{
        Store, audit_head,
        control::{
            self,
            supervision::{Input, Record},
        },
        worker_transition::{ObservationClass, classify_observation},
    },
    control::{
        AllocationV1, ArmSubjectV1, ArmV1, HelperObservationV1, RuntimeObservationV1,
        RuntimeStatusV1, WorkerStateV1, WorkerV1,
    },
    model::{Event, Outcome},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

pub const SEAL_SELECTION_FORMAT: &str = "ai-guard.audit.seal-selection.v1";

/// The broker Full journal head of one operation, as the maintenance caller
/// reads it from the journal entry. Every field mirrors what an accepted,
/// settled entry reports; a missing or divergent field refuses the selection.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealBrokerHeadV1 {
    pub operation_id: String,
    pub head_revision: Option<u64>,
    pub head_hash: Option<String>,
    pub worker_state: Option<WorkerStateV1>,
    pub quota_held: Option<bool>,
    pub runtime_status: Option<RuntimeStatusV1>,
    pub pending_rpc_count: Option<usize>,
    pub cancel_requested: bool,
}

/// One ordered proof reference of the sealed worker: its chain position, its
/// hash, and the audited action it proves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealProofRefV1 {
    pub seq: i64,
    pub hash: String,
    pub kind: String,
    pub action: String,
    pub operation_id: Option<String>,
}

/// The read-only result of a successful selection. No publication format is
/// decided here; the dossier interfaces come with the sealing slice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedWorkerSelectionV1 {
    pub format: String,
    pub tenant_id: String,
    pub worker_id: String,
    pub attempt_id: String,
    pub store_head_seq: i64,
    pub store_head_hash: String,
    pub broker_operation_id: String,
    pub broker_head_revision: u64,
    pub broker_head_hash: String,
    pub terminal_observation_id: String,
    pub allocation: AllocationV1,
    pub references: Vec<SealProofRefV1>,
}

fn incomplete(reason: &str) -> RetentionFailure {
    RetentionFailure::new(
        code::SEAL_CLOSURE_INCOMPLETE,
        "the worker closure is incomplete; nothing was changed",
    )
    .with("reason", reason)
}

fn broker_head_refusal(reason: &str) -> RetentionFailure {
    RetentionFailure::new(
        code::SEAL_BROKER_HEAD_INCOMPLETE,
        "the broker Full journal head of this operation is incomplete or divergent",
    )
    .with("reason", reason)
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Cross-check the broker head against the worker's claimed start operation.
/// The journal must name the same operation, carry a published head, and
/// report the same terminal, quota-released, exited state the Store proves.
fn check_broker_head(head: &SealBrokerHeadV1, worker: &WorkerV1) -> Result<(), RetentionFailure> {
    let start = worker
        .attempt
        .start_operation_id
        .as_deref()
        .ok_or_else(|| incomplete("start_unclaimed"))?;
    if head.operation_id != start {
        return Err(broker_head_refusal("other_operation"));
    }
    if !head.head_revision.is_some_and(|revision| revision >= 1)
        || !head.head_hash.as_deref().is_some_and(valid_hash)
    {
        return Err(broker_head_refusal("missing_head"));
    }
    if head.worker_state != Some(WorkerStateV1::Terminal) {
        return Err(broker_head_refusal("divergent_state"));
    }
    if head.quota_held != Some(false) {
        return Err(broker_head_refusal("held_quota"));
    }
    if head.runtime_status != Some(RuntimeStatusV1::Exited) {
        return Err(broker_head_refusal("divergent_runtime"));
    }
    if head.pending_rpc_count != Some(0) {
        return Err(broker_head_refusal("uncertain_publication"));
    }
    if head.cancel_requested {
        return Err(broker_head_refusal("cancelled"));
    }
    Ok(())
}

fn event_at(db: &Connection, seq: i64) -> anyhow::Result<(String, String, Event)> {
    let (kind, hash, payload): (String, String, String) = db.query_row(
        "SELECT kind,hash,payload FROM events WHERE seq=?1",
        [seq],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let event: Event = serde_json::from_str(&payload)?;
    Ok((kind, hash, event))
}

fn terminal_helper_observation(observation: &RuntimeObservationV1) -> bool {
    matches!(
        observation.status,
        RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
    ) && observation.running == Some(false)
        && observation.exit_code.is_some()
        && observation.started_at.is_some()
        && observation.finished_at.is_some()
}

struct MutationRow {
    operation_id: String,
    action: String,
    seq: i64,
}

fn mutations_of(db: &Connection, worker_id: &str) -> anyhow::Result<Vec<MutationRow>> {
    let mut statement = db.prepare(
        "SELECT operation_id,action,event_seq FROM worker_mutations WHERE worker_id=?1 ORDER BY event_seq",
    )?;
    let rows = statement
        .query_map([worker_id], |r| {
            Ok(MutationRow {
                operation_id: r.get(0)?,
                action: r.get(1)?,
                seq: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

impl Store {
    /// Read-only selection of one closed supervised worker for sealing.
    ///
    /// Refuses (without any effect) when the closure is incomplete, the
    /// broker head of the same operation is incomplete or divergent, the
    /// chain does not verify, or the Store head drifts inside the snapshot.
    pub fn select_sealed_worker(
        &self,
        worker_id: &str,
        broker_head: &SealBrokerHeadV1,
    ) -> anyhow::Result<SealedWorkerSelectionV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        self.backing.verify(&tx)?;
        // One validated snapshot: the live chain, then the complete control
        // and supervision replay that proves every projection from the audit.
        let integrity = Self::check_integrity(&tx)?;
        if !integrity.valid {
            return Err(RetentionFailure::new(
                code::AUDIT_CHAIN_INVALID,
                "the live audit chain does not verify",
            )
            .with("integrity_error", integrity.error.clone())
            .into());
        }
        control::validate(&tx)?;
        let (head_seq, head_hash) = audit_head(&tx)?;

        let snapshot: Option<String> = tx
            .query_row(
                "SELECT snapshot_json FROM workers WHERE worker_id=?1",
                [worker_id],
                |r| r.get(0),
            )
            .optional()?;
        let worker: WorkerV1 = match snapshot {
            Some(json) => serde_json::from_str(&json)?,
            None => {
                return Err(RetentionFailure::new(
                    code::SEAL_WORKER_NOT_FOUND,
                    "this tenant store has no such worker",
                )
                .with("worker_id", worker_id)
                .into());
            }
        };

        // Closure of the worker itself.
        if worker.state != WorkerStateV1::Terminal {
            return Err(incomplete("worker_not_terminal").into());
        }
        if worker.quota_held {
            return Err(incomplete("quota_still_held").into());
        }
        if worker.cleanup_removed != Some(true) {
            return Err(incomplete("cleanup_unconfirmed").into());
        }
        if worker.cleanup_error.is_some() {
            return Err(incomplete("cleanup_error").into());
        }
        if worker.allocation_inconsistent {
            return Err(incomplete("allocation_inconsistent").into());
        }
        let inspection = worker
            .attempt
            .created_inspection
            .as_ref()
            .ok_or_else(|| incomplete("container_unknown"))?;
        let container_id = inspection.container_id.clone();

        // The broker Full journal head of the same operation.
        check_broker_head(broker_head, &worker)?;

        let mutations = mutations_of(&tx, worker_id)?;
        // Any export grant leaves the closure incomplete: the export
        // destination has no confirmed publication yet.
        if let Some(export) = mutations.iter().find(|m| m.action == "export_request") {
            return Err(incomplete("pending_export")
                .with("operation_id", export.operation_id.clone())
                .into());
        }
        // The admission and the consumed start claim are correlated receipts.
        let admission = mutations
            .iter()
            .find(|m| {
                m.action == "supervised_admit" && m.operation_id == worker.admission_operation_id
            })
            .ok_or_else(|| incomplete("admission_missing"))?;
        let start_operation = worker
            .attempt
            .start_operation_id
            .as_deref()
            .ok_or_else(|| incomplete("start_unclaimed"))?;
        let start_claim = mutations
            .iter()
            .find(|m| m.action == "armed_start_claim" && m.operation_id == start_operation)
            .ok_or_else(|| incomplete("start_claim_missing"))?;
        let (_, _, claim_event) = event_at(&tx, start_claim.seq)?;
        let claim_record: Record = serde_json::from_value(claim_event.data.clone())?;
        let Input::WorkerClaim(claim_request) = claim_record.input else {
            return Err(incomplete("start_claim_missing").into());
        };
        if !mutations.iter().any(|m| m.action == "cleanup_result") {
            return Err(incomplete("cleanup_unconfirmed").into());
        }
        // The admission receipt carries the exact decision receipt.
        let (_, _, admission_event) = event_at(&tx, admission.seq)?;
        let admission_record: Record = serde_json::from_value(admission_event.data.clone())?;
        let linked_decision = admission_record
            .worker_record
            .as_ref()
            .and_then(|record| record.worker.as_ref())
            .map(|worker| serde_json::to_value(&worker.receipt))
            .transpose()?;
        if linked_decision != Some(serde_json::to_value(&worker.receipt)?) {
            return Err(incomplete("admission_decision_unlinked").into());
        }
        // Helper cleanups, decoded once from their audited receipts.
        let mut helper_cleanups = Vec::new();
        for mutation in mutations.iter().filter(|m| m.action == "helper_cleanup") {
            let (_, _, event) = event_at(&tx, mutation.seq)?;
            let record: Record = serde_json::from_value(event.data.clone())?;
            if let Input::HelperCleanup(request) = record.input {
                helper_cleanups.push(*request);
            }
        }

        // The decision receipt links to an allowed decision event.
        let (decision_kind, decision_hash, decision_event) =
            event_at(&tx, worker.receipt.audit_seq)?;
        if decision_kind != "security.decision" || decision_hash != worker.receipt.audit_hash {
            return Err(incomplete("decision_receipt_diverges").into());
        }
        if decision_event.decision.as_ref().map(|d| &d.outcome) != Some(&Outcome::Allow) {
            return Err(incomplete("decision_not_allowed").into());
        }

        // Every arm of this attempt: consumed claim, terminal observation and
        // confirmed cleanup for helpers; the worker arm binds the start claim.
        let mut statement = tx.prepare(
            "SELECT arm_id,attempt_id,payload_json FROM execution_arms WHERE worker_id=?1 ORDER BY event_seq",
        )?;
        let arms = statement
            .query_map([worker_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut worker_arm_seen = false;
        for (arm_id, attempt_id, payload) in &arms {
            if attempt_id != &worker.attempt.attempt_id {
                return Err(incomplete("arm_attempt_diverges").into());
            }
            let arm: ArmV1 = serde_json::from_str(payload)?;
            match &arm.subject {
                ArmSubjectV1::Worker => {
                    worker_arm_seen = true;
                    if claim_request.arm_receipt.arm_id != *arm_id {
                        return Err(incomplete("start_claim_arm_diverges").into());
                    }
                }
                ArmSubjectV1::Helper { helper_id, .. } => {
                    let claimed: Option<String> = tx
                        .query_row(
                            "SELECT operation_id FROM helper_claims WHERE arm_id=?1",
                            [arm_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if claimed.is_none() {
                        return Err(incomplete("helper_claim_missing")
                            .with("helper_id", helper_id.clone())
                            .into());
                    }
                    let mut statement = tx.prepare(
                        "SELECT payload_json FROM helper_observations WHERE helper_id=?1 ORDER BY event_seq",
                    )?;
                    let observations = statement
                        .query_map([helper_id], |r| r.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<String>>>()?;
                    drop(statement);
                    let terminal = observations
                        .iter()
                        .map(|json| serde_json::from_str::<HelperObservationV1>(json))
                        .collect::<Result<Vec<_>, _>>()?
                        .into_iter()
                        .any(|o| terminal_helper_observation(&o.observation));
                    if !terminal {
                        return Err(incomplete("helper_observation_missing")
                            .with("helper_id", helper_id.clone())
                            .into());
                    }
                    let cleanup = helper_cleanups
                        .iter()
                        .find(|request| request.helper_id == *helper_id);
                    match cleanup {
                        Some(request) if request.removed && request.error.is_none() => {}
                        _ => {
                            return Err(incomplete("helper_cleanup_unconfirmed")
                                .with("helper_id", helper_id.clone())
                                .into());
                        }
                    }
                }
            }
        }
        if !worker_arm_seen {
            return Err(incomplete("worker_arm_missing").into());
        }

        // The unique non-billable allocation, linked to the terminal
        // observation of the same container.
        let allocation_row: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT payload_json,terminal_observation_id,event_seq FROM worker_allocations WHERE attempt_id=?1 AND worker_id=?2",
                [worker.attempt.attempt_id.as_str(), worker_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (allocation_json, terminal_observation_id, allocation_seq) =
            allocation_row.ok_or_else(|| incomplete("allocation_missing"))?;
        let allocation: AllocationV1 = serde_json::from_str(&allocation_json)?;
        if allocation.billable {
            return Err(incomplete("allocation_billable").into());
        }
        if allocation.source.container_id != container_id {
            return Err(incomplete("allocation_container_diverges").into());
        }
        let (observation_json, observation_seq): (String, i64) = tx
            .query_row(
                "SELECT payload_json,event_seq FROM worker_observations WHERE observation_id=?1",
                [&terminal_observation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| incomplete("terminal_observation_missing"))?;
        let observation: RuntimeObservationV1 = serde_json::from_str(&observation_json)?;
        if !matches!(
            classify_observation(&worker, &observation),
            ObservationClass::Terminal(..)
        ) {
            return Err(incomplete("terminal_observation_missing").into());
        }
        if observation.inspection.container_id != container_id {
            return Err(incomplete("terminal_container_diverges").into());
        }
        if allocation_seq != observation_seq {
            return Err(incomplete("allocation_observation_unlinked").into());
        }
        if allocation.terminal_observation_id != terminal_observation_id {
            return Err(incomplete("allocation_observation_unlinked").into());
        }
        // The terminal observation is an audited observe mutation receipt.
        if !mutations.iter().any(|m| {
            m.action == "observe"
                && m.seq == observation_seq
                && m.operation_id == terminal_observation_id
        }) {
            return Err(incomplete("terminal_observation_missing").into());
        }

        // The exact ordered reference set: the decision, then every audited
        // mutation of this worker from admission to confirmed cleanup.
        let mut references = vec![SealProofRefV1 {
            seq: worker.receipt.audit_seq,
            hash: decision_hash,
            kind: decision_kind,
            action: "decision".into(),
            operation_id: None,
        }];
        let mut statement = tx.prepare(
            "SELECT e.seq,e.hash,e.kind,m.operation_id,m.action FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.worker_id=?1 ORDER BY e.seq",
        )?;
        let rows = statement
            .query_map([worker_id], |r| {
                Ok(SealProofRefV1 {
                    seq: r.get(0)?,
                    hash: r.get(1)?,
                    kind: r.get(2)?,
                    action: r.get(4)?,
                    operation_id: Some(r.get(3)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for reference in rows {
            if reference.seq > head_seq {
                return Err(incomplete("proof_beyond_head").into());
            }
            if references
                .last()
                .is_some_and(|last| last.seq >= reference.seq)
            {
                return Err(incomplete("proof_order_diverges").into());
            }
            references.push(reference);
        }
        // Revalidate the Store head under the same snapshot before returning.
        let (final_seq, final_hash) = audit_head(&tx)?;
        if final_seq != head_seq || final_hash != head_hash {
            return Err(RetentionFailure::new(
                code::SEAL_HEAD_DRIFT,
                "the store head drifted during selection; redo the selection",
            )
            .into());
        }

        Ok(SealedWorkerSelectionV1 {
            format: SEAL_SELECTION_FORMAT.into(),
            tenant_id: self.tenant_id.clone(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            store_head_seq: head_seq,
            store_head_hash: head_hash,
            broker_operation_id: broker_head.operation_id.clone(),
            broker_head_revision: broker_head.head_revision.unwrap_or(0),
            broker_head_hash: broker_head.head_hash.clone().unwrap_or_default(),
            terminal_observation_id,
            allocation,
            references,
        })
    }
}
