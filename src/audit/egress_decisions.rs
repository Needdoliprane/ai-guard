//! Egress journal of one tenant store (SQLite adapter).
//!
//! Every fact goes into the generic hash-chained `events` table under the
//! reserved `egress.` namespace: no new table or format. Each mutation runs
//! in one checked transaction; idempotency keys are the event ids, so the
//! same id with the same content returns the original receipt and a
//! different content is a conflict. Pure evaluation lives in
//! `control::egress_decision` and is shared with the MySQL adapter.

use super::{CheckedTransaction, Store, row_event};
use crate::{
    control::{
        WorkerStateV1, WorkerV1,
        egress_decision::{
            Decided, EgressCloseReasonV1, EgressDecideRequestV1, EgressDecisionV1, EgressGrantV1,
            EgressObservationV1, EgressObserveRequestV1, EgressPolicySnapshot, EgressReceiptV1,
            EgressRenewRequestV1, EgressRenewResponseV1, WorkerFacts, decide, renewal,
        },
    },
    egress::{Authority, ids::IdV1},
    model::{Event, StoredEvent},
};
use serde_json::{Value, json};

pub const NAMESPACE: &str = "egress";
pub const DECISION_KIND: &str = "egress.decision";
pub const GRANT_ISSUED_KIND: &str = "egress.grant.issued";
pub const GRANT_REVOKED_KIND: &str = "egress.grant.revoked";
/// Bound on grant facts replayed for one session inside a transaction.
pub const MAX_SESSION_GRANT_EVENTS: usize = 1_024;
const MAX_RENEW_DECISIONS: usize = 16;

/// Typed refusal of a journal mutation, mapped to HTTP by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressJournalError {
    /// Same idempotency key, different content.
    Conflict,
    /// Malformed or unbound request.
    Invalid,
}
impl std::fmt::Display for EgressJournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Conflict => "egress_conflict",
            Self::Invalid => "egress_invalid",
        })
    }
}
impl std::error::Error for EgressJournalError {}

pub fn observation_kind(observation: &EgressObservationV1) -> &'static str {
    match observation {
        EgressObservationV1::ChannelOpened { .. } => "egress.channel.opened",
        EgressObservationV1::ChannelClosed { .. } => "egress.channel.closed",
        EgressObservationV1::Connection { .. } => "egress.connection",
        EgressObservationV1::Closed { .. } => "egress.closed",
        EgressObservationV1::DirectAttempts { .. } => "egress.direct_attempts",
    }
}

fn binding_of(
    observation: &EgressObservationV1,
) -> &crate::control::egress_decision::EgressBindingV1 {
    match observation {
        EgressObservationV1::ChannelOpened { binding }
        | EgressObservationV1::ChannelClosed { binding, .. }
        | EgressObservationV1::Connection { binding, .. }
        | EgressObservationV1::Closed { binding, .. }
        | EgressObservationV1::DirectAttempts { binding, .. } => binding,
    }
}

fn receipt(stored: &StoredEvent) -> EgressReceiptV1 {
    EgressReceiptV1 {
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}

fn event(kind: &str, id: &IdV1, session: &str, worker: &str, data: Value) -> Event {
    let mut event = Event::new(kind);
    event.id = id.as_str().into();
    event.source = "gateway".into();
    event.provenance = "observed".into();
    // The journal observes the mediated channel without proving direct-attempt coverage.
    event.coverage = "partial".into();
    event.session_id = Some(session.into());
    event.sandbox_id = Some(worker.into());
    event.data = data;
    event
}

/// One event by id (the idempotency key), restricted to the egress kinds.
fn by_id(tx: &CheckedTransaction<'_>, id: &IdV1) -> anyhow::Result<Option<StoredEvent>> {
    let mut statement = tx.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1 AND kind LIKE 'egress.%'",
    )?;
    let mut rows = statement.query_map([id.as_str()], row_event)?;
    rows.next().transpose().map_err(Into::into)
}

fn worker_facts(
    tx: &CheckedTransaction<'_>,
    request: &EgressDecideRequestV1,
) -> anyhow::Result<WorkerFacts> {
    let snapshot: Option<String> = tx
        .query_row(
            "SELECT snapshot_json FROM workers WHERE worker_id=?1",
            [&request.binding.worker_id],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(snapshot) = snapshot else {
        return Ok(WorkerFacts::BindingMismatch);
    };
    let worker: WorkerV1 = serde_json::from_str(&snapshot)?;
    Ok(
        if worker.launch.session_id != request.binding.session_id
            || worker.attempt.attempt_id != request.binding.attempt_id
        {
            WorkerFacts::BindingMismatch
        } else if worker.launch.egress.is_none() {
            WorkerFacts::EgressNotLaunched
        } else if worker.state != WorkerStateV1::Running {
            WorkerFacts::NotRunning
        } else {
            WorkerFacts::Eligible
        },
    )
}

/// Replay the grant facts of one session, bounded, in journal order.
fn session_grants(
    tx: &CheckedTransaction<'_>,
    session: &str,
) -> anyhow::Result<Vec<EgressGrantV1>> {
    let mut statement = tx.prepare(
        "SELECT kind,payload FROM events WHERE kind IN (?1,?2) AND json_extract(payload,'$.session_id')=?3 ORDER BY seq LIMIT ?4",
    )?;
    let rows = statement
        .query_map(
            rusqlite::params![
                GRANT_ISSUED_KIND,
                GRANT_REVOKED_KIND,
                session,
                MAX_SESSION_GRANT_EVENTS as i64 + 1
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        rows.len() <= MAX_SESSION_GRANT_EVENTS,
        "session grant history exceeds its bound"
    );
    let mut grants: Vec<EgressGrantV1> = Vec::new();
    for (kind, payload) in rows {
        let event: Event = serde_json::from_str(&payload)?;
        if kind == GRANT_ISSUED_KIND {
            grants.push(serde_json::from_value(event.data["grant"].clone())?);
        } else {
            let id: IdV1 = serde_json::from_value(event.data["grant_id"].clone())?;
            let at = event.data["revoked_at_ms"]
                .as_i64()
                .ok_or(EgressJournalError::Invalid)?;
            if let Some(grant) = grants.iter_mut().find(|grant| grant.grant_id == id) {
                grant.revoked_at_ms.get_or_insert(at);
            }
        }
    }
    Ok(grants)
}

fn stored_decision(
    stored: &StoredEvent,
) -> anyhow::Result<(EgressDecideRequestV1, EgressDecisionV1)> {
    let request: EgressDecideRequestV1 =
        serde_json::from_value(stored.event.data["request"].clone())?;
    let mut decision: EgressDecisionV1 =
        serde_json::from_value(stored.event.data["decision"].clone())?;
    decision.receipt = receipt(stored);
    Ok((request, decision))
}

impl Store {
    /// Select the tenant's immutable operator egress policy before sharing the
    /// Store. Without one, every egress decision is a durable deny.
    pub fn with_egress_policy(
        mut self,
        policy: &crate::control::egress_decision::EgressTenantPolicyV1,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            std::sync::Arc::strong_count(&self.supervision_lease) == 1,
            "egress policy must be selected before sharing the Store"
        );
        self.egress_policy = Some(policy.snapshot()?);
        Ok(self)
    }
    /// Read a closed operator policy from a private, bounded file. Validated
    /// before any Store opens; nothing is reloaded later.
    pub fn load_egress_policy(
        path: &std::path::Path,
    ) -> anyhow::Result<crate::control::egress_decision::EgressTenantPolicyV1> {
        let file = crate::control_fs::PrivateFile::open(path, 64 * 1024)?;
        let policy: crate::control::egress_decision::EgressTenantPolicyV1 =
            serde_json::from_slice(file.bytes())?;
        policy.snapshot()?;
        Ok(policy)
    }
    /// The effective policy snapshot: the configured one, or default deny.
    pub fn egress_policy(&self) -> EgressPolicySnapshot {
        self.egress_policy
            .clone()
            .unwrap_or_else(EgressPolicySnapshot::default_deny)
    }
    /// Durable egress decision before any resolution or connect. The tenant
    /// is this store; the worker must be Running in it with broker egress.
    pub fn egress_decide(
        &self,
        request: &EgressDecideRequestV1,
        policy: &EgressPolicySnapshot,
        now_ms: i64,
    ) -> anyhow::Result<EgressDecisionV1> {
        anyhow::ensure!(request.version == 1, EgressJournalError::Invalid);
        let authority = Authority::parse_connect(&request.authority)
            .map_err(|_| EgressJournalError::Invalid)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(existing) = by_id(&tx, &request.decision_id)? {
            let (stored_request, decision) = stored_decision(&existing)?;
            anyhow::ensure!(
                existing.event.kind == DECISION_KIND && &stored_request == request,
                EgressJournalError::Conflict
            );
            self.commit_checked(tx)?;
            return Ok(decision);
        }
        let facts = worker_facts(&tx, request)?;
        let grants = session_grants(&tx, &request.binding.session_id)?;
        let Decided {
            outcome,
            reason,
            grant_id,
            remaining_ms,
        } = decide(
            policy,
            &grants,
            facts,
            &request.binding.session_id,
            &authority,
            now_ms,
        );
        let mut decision = EgressDecisionV1 {
            version: 1,
            decision_id: request.decision_id.clone(),
            outcome,
            reason,
            policy: Some(policy.reference()),
            grant_id,
            remaining_ms,
            receipt: EgressReceiptV1 {
                audit_seq: 0,
                audit_hash: String::new(),
            },
        };
        let data = json!({"request": request, "decision": decision, "decided_at_ms": now_ms});
        let event = event(
            DECISION_KIND,
            &request.decision_id,
            &request.binding.session_id,
            &request.binding.worker_id,
            data,
        );
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        decision.receipt = receipt(&stored);
        Ok(decision)
    }

    /// Broker observation, idempotent by observation id.
    pub fn egress_observe(
        &self,
        request: &EgressObserveRequestV1,
    ) -> anyhow::Result<EgressReceiptV1> {
        anyhow::ensure!(request.version == 1, EgressJournalError::Invalid);
        let kind = observation_kind(&request.observation);
        let binding = binding_of(&request.observation);
        let data = serde_json::to_value(&request.observation)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(existing) = by_id(&tx, &request.observation_id)? {
            anyhow::ensure!(
                existing.event.kind == kind && existing.event.data == data,
                EgressJournalError::Conflict
            );
            self.commit_checked(tx)?;
            return Ok(receipt(&existing));
        }
        let event = event(
            kind,
            &request.observation_id,
            &binding.session_id,
            &binding.worker_id,
            data,
        );
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        Ok(receipt(&stored))
    }

    /// Read-only revalidation of live decisions against current durable state.
    pub fn egress_renew(
        &self,
        request: &EgressRenewRequestV1,
        policy: &EgressPolicySnapshot,
        now_ms: i64,
    ) -> anyhow::Result<EgressRenewResponseV1> {
        anyhow::ensure!(
            request.version == 1 && request.decisions.len() <= MAX_RENEW_DECISIONS,
            EgressJournalError::Invalid
        );
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let grants = session_grants(&tx, &request.binding.session_id)?;
        let (mut revoked, mut expired) = (Vec::new(), Vec::new());
        for id in &request.decisions {
            let verdict = match by_id(&tx, id)? {
                Some(stored) if stored.event.kind == DECISION_KIND => {
                    let (stored_request, decision) = stored_decision(&stored)?;
                    if stored_request.binding != request.binding {
                        Some(EgressCloseReasonV1::Revoked)
                    } else {
                        let authority = Authority::parse_connect(&stored_request.authority)
                            .map_err(|_| EgressJournalError::Invalid)?;
                        let decided = Decided {
                            outcome: decision.outcome,
                            reason: decision.reason,
                            grant_id: decision.grant_id.clone(),
                            remaining_ms: decision.remaining_ms,
                        };
                        let grant = decision.grant_id.as_ref().and_then(|grant_id| {
                            grants.iter().find(|grant| &grant.grant_id == grant_id)
                        });
                        renewal(policy, &decided, grant, &authority, now_ms)
                    }
                }
                // An unknown decision never keeps a flow open.
                _ => Some(EgressCloseReasonV1::Revoked),
            };
            match verdict {
                None => {}
                Some(EgressCloseReasonV1::Expired) => expired.push(id.clone()),
                Some(_) => revoked.push(id.clone()),
            }
        }
        self.commit_checked(tx)?;
        Ok(EgressRenewResponseV1 {
            version: 1,
            revoked,
            expired,
        })
    }

    /// Bounded egress facts of one session, oldest first, for the read API.
    pub fn egress_session_events(
        &self,
        session: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredEvent>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let events = {
            let mut statement = tx.prepare(
                "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind LIKE 'egress.%' AND json_extract(payload,'$.session_id')=?1 ORDER BY seq DESC LIMIT ?2",
            )?;
            statement
                .query_map(
                    rusqlite::params![session, limit.min(1_000) as i64],
                    row_event,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(events.into_iter().rev().collect())
    }
}

#[cfg(test)]
#[path = "egress_decisions_tests.rs"]
mod tests;
