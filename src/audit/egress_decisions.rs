//! Egress journal of one tenant store (SQLite adapter).
//!
//! Every fact goes into the generic hash-chained `events` table under the
//! reserved `egress.` namespace: no new table or format. Each mutation runs
//! in one checked transaction; idempotency keys are the event ids, so the
//! same id with the same content returns the original receipt and a
//! different content is a conflict. Pure evaluation lives in
//! `control::egress_decision` and is shared with the MySQL adapter.

use super::{CheckedTransaction, Store, row_event, workload};
use crate::{
    control::{
        WorkerStateV1, WorkerV1,
        egress_decision::{
            Decided, EgressBindingV1, EgressChannelOpeningDispositionV1,
            EgressChannelOpeningReceiptV1, EgressChannelOpeningRequestV1, EgressCloseReasonV1,
            EgressConnectPermitDispositionV1, EgressConnectPermitReceiptV1,
            EgressConnectPermitRequestV1, EgressDecideRequestV1, EgressDecisionV1, EgressGrantV1,
            EgressObservationV1, EgressObserveRequestV1, EgressPolicySnapshot, EgressReceiptV1,
            EgressRenewRequestV1, EgressRenewResponseV1, MAX_CONNECT_PERMIT_MS, WorkerFacts,
            decide, deserialize_object, renewal,
        },
    },
    egress::{Authority, PolicyLimits, ValidatedResolution, ids::IdV1},
    model::{Event, StoredEvent},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::SocketAddr;

pub const NAMESPACE: &str = "egress";
pub const DECISION_KIND: &str = "egress.decision";
const CONNECT_PERMIT_KIND: &str = "egress.connect.permit";
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

fn binding_of(observation: &EgressObservationV1) -> &EgressBindingV1 {
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
    binding: &EgressBindingV1,
) -> anyhow::Result<WorkerFacts> {
    let snapshot: Option<String> = tx
        .query_row(
            "SELECT snapshot_json FROM workers WHERE worker_id=?1",
            [&binding.worker_id],
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
        if worker.worker_id != binding.worker_id
            || worker.launch.session_id != binding.session_id
            || worker.attempt.attempt_id != binding.attempt_id
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

fn channel_opening_seq(
    tx: &CheckedTransaction<'_>,
    opening_id: &IdV1,
    binding: &EgressBindingV1,
) -> anyhow::Result<i64> {
    let opening = by_id(tx, opening_id)?
        .filter(|stored| stored.event.kind == "egress.channel.opened")
        .ok_or(EgressJournalError::Invalid)?;
    let observation: EgressObservationV1 = serde_json::from_value(opening.event.data)?;
    anyhow::ensure!(
        matches!(observation, EgressObservationV1::ChannelOpened { binding: recorded } if &recorded == binding),
        EgressJournalError::Invalid
    );
    Ok(opening.seq)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConnectPermitRecord {
    #[serde(deserialize_with = "deserialize_object")]
    request: EgressConnectPermitRequestV1,
    port: u16,
    issued_at_ms: i64,
    expires_at_ms: i64,
}

impl ConnectPermitRecord {
    fn into_receipt(
        self,
        stored: &StoredEvent,
        disposition: EgressConnectPermitDispositionV1,
    ) -> anyhow::Result<EgressConnectPermitReceiptV1> {
        let valid_for_ms = self
            .expires_at_ms
            .checked_sub(self.issued_at_ms)
            .and_then(|ms| u64::try_from(ms).ok())
            .filter(|ms| (1..=MAX_CONNECT_PERMIT_MS).contains(ms))
            .ok_or(EgressJournalError::Invalid)?;
        Ok(EgressConnectPermitReceiptV1 {
            version: 1,
            permit_id: self.request.permit_id()?,
            request: self.request,
            port: self.port,
            valid_for_ms,
            disposition,
            receipt: receipt(stored),
        })
    }
}

fn require_connected_permit(
    tx: &CheckedTransaction<'_>,
    name: &EgressDecideRequestV1,
    decision_seq: i64,
    peer: &str,
) -> anyhow::Result<()> {
    let permit_id = EgressConnectPermitRequestV1::permit_id_for(&name.binding, name.flow_id)?;
    let stored = by_id(tx, &permit_id)?
        .filter(|stored| stored.event.kind == CONNECT_PERMIT_KIND && stored.seq > decision_seq)
        .ok_or(EgressJournalError::Invalid)?;
    let permit: ConnectPermitRecord = serde_json::from_value(stored.event.data)?;
    let request = permit.request;
    anyhow::ensure!(
        request.version == 1
            && request.decision_id == name.decision_id
            && request.binding == name.binding
            && request.flow_id == name.flow_id
            && permit.port != 0
            && SocketAddr::new(request.ip, permit.port).to_string() == peer
            && channel_opening_seq(tx, &request.opening_id, &request.binding)? < decision_seq,
        EgressJournalError::Invalid
    );
    Ok(())
}

impl Store {
    fn prepare_egress_event(&self, event: Event) -> anyhow::Result<(Event, String)> {
        // The event has a canonical timestamp and its typed payload is the
        // input to exact replay. Refuse a collision before any record is written.
        let original = serde_json::to_value(&event)?;
        let (event, payload) = self.prepare_event(event)?;
        anyhow::ensure!(
            serde_json::to_value(&event)? == original,
            EgressJournalError::Invalid
        );
        Ok((event, payload))
    }

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
        let facts = worker_facts(&tx, &request.binding)?;
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
        let (event, payload) = self.prepare_egress_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        decision.receipt = receipt(&stored);
        Ok(decision)
    }

    /// Commit an exact numeric dial permit for a live Legacy SQLite opening.
    /// A replay reads history only, including after the opening is no longer live.
    pub fn egress_issue_connect_permit(
        &self,
        request: &EgressConnectPermitRequestV1,
    ) -> anyhow::Result<EgressConnectPermitReceiptV1> {
        anyhow::ensure!(
            request.version == 1
                && request.flow_id > 0
                && request.binding.epoch > 0
                && request.ip == request.ip.to_canonical(),
            EgressJournalError::Invalid
        );
        let permit_id = request.permit_id()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(stored) = by_id(&tx, &permit_id)? {
            anyhow::ensure!(
                stored.event.kind == CONNECT_PERMIT_KIND,
                EgressJournalError::Conflict
            );
            let record: ConnectPermitRecord = serde_json::from_value(stored.event.data.clone())?;
            anyhow::ensure!(&record.request == request, EgressJournalError::Conflict);
            let replay =
                record.into_receipt(&stored, EgressConnectPermitDispositionV1::Replayed)?;
            self.commit_checked(tx)?;
            return Ok(replay);
        }

        let opening_seq = channel_opening_seq(&tx, &request.opening_id, &request.binding)?;
        anyhow::ensure!(
            worker_facts(&tx, &request.binding)? == WorkerFacts::Eligible,
            EgressJournalError::Invalid
        );
        let epoch =
            i64::try_from(request.binding.epoch).map_err(|_| EgressJournalError::Invalid)?;
        let closed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE kind='egress.channel.closed' \
             AND json_extract(payload,'$.data.binding.channel_id')=?1 \
             AND json_extract(payload,'$.data.binding.epoch')=?2)",
            rusqlite::params![request.binding.channel_id.as_str(), epoch],
            |row| row.get(0),
        )?;
        anyhow::ensure!(!closed, EgressJournalError::Invalid);

        let stored = by_id(&tx, &request.decision_id)?
            .filter(|stored| stored.event.kind == DECISION_KIND && stored.seq > opening_seq)
            .ok_or(EgressJournalError::Invalid)?;
        let (name, decision) = stored_decision(&stored)?;
        let policy = self.egress_policy();
        anyhow::ensure!(
            name.decision_id == request.decision_id
                && name.binding == request.binding
                && name.flow_id == request.flow_id
                && decision.allows(&name)
                && decision.policy.as_ref() == Some(&policy.reference()),
            EgressJournalError::Invalid
        );
        let authority =
            Authority::parse_connect(&name.authority).map_err(|_| EgressJournalError::Invalid)?;
        ValidatedResolution::new(&authority, &[request.ip], PolicyLimits::new(1, 1)?)
            .map_err(|_| EgressJournalError::Invalid)?;

        // Old Legacy formats without the durable UTC floor cannot issue permits.
        anyhow::ensure!(
            workload::time_floors_present(&tx)?,
            EgressJournalError::Invalid
        );
        let grants = session_grants(&tx, &request.binding.session_id)?;
        let grant = decision.grant_id.as_ref().and_then(|id| {
            grants.iter().find(|grant| {
                &grant.grant_id == id
                    && grant.session_id == request.binding.session_id
                    && grant.authority == name.authority
            })
        });
        let clock = workload::capture_control_time(&tx)?;
        let issued_at_ms = clock.at().timestamp_millis();
        let decided_at_ms = stored.event.data["decided_at_ms"]
            .as_i64()
            .filter(|at| *at <= issued_at_ms)
            .ok_or(EgressJournalError::Invalid)?;
        let name_expires_at_ms = decision
            .remaining_ms
            .and_then(|ms| i64::try_from(ms).ok())
            .and_then(|ms| decided_at_ms.checked_add(ms))
            .ok_or(EgressJournalError::Invalid)?;
        let decided = Decided {
            outcome: decision.outcome,
            reason: decision.reason,
            grant_id: decision.grant_id,
            remaining_ms: decision.remaining_ms,
        };
        anyhow::ensure!(
            renewal(&policy, &decided, grant, &authority, issued_at_ms).is_none(),
            EgressJournalError::Invalid
        );
        let expires_at_ms = grant
            .map_or(name_expires_at_ms, |grant| {
                name_expires_at_ms.min(grant.expires_at_ms)
            })
            .min(
                issued_at_ms
                    .checked_add(MAX_CONNECT_PERMIT_MS as i64)
                    .ok_or(EgressJournalError::Invalid)?,
            );
        anyhow::ensure!(expires_at_ms > issued_at_ms, EgressJournalError::Invalid);
        let record = ConnectPermitRecord {
            request: request.clone(),
            port: authority.port(),
            issued_at_ms,
            expires_at_ms,
        };
        let mut event = event(
            CONNECT_PERMIT_KIND,
            &permit_id,
            &request.binding.session_id,
            &request.binding.worker_id,
            serde_json::to_value(&record)?,
        );
        event.timestamp = crate::broker_registry::format_time(clock.at());
        let (event, payload) = self.prepare_egress_event(event)?;
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        anyhow::ensure!(
            Utc::now().timestamp_millis() < expires_at_ms,
            EgressJournalError::Invalid
        );
        self.commit_authorized(tx, &clock)?;
        // A commit may outlast its window. Keep its history without granting a dial.
        anyhow::ensure!(
            (issued_at_ms..expires_at_ms).contains(&Utc::now().timestamp_millis()),
            EgressJournalError::Invalid
        );
        record.into_receipt(&stored, EgressConnectPermitDispositionV1::Fresh)
    }

    /// Broker observation, idempotent by observation id.
    pub fn egress_observe(
        &self,
        request: &EgressObserveRequestV1,
    ) -> anyhow::Result<EgressReceiptV1> {
        self.record_egress_observation(
            request.version,
            &request.observation_id,
            &request.observation,
        )
        .map(|(receipt, _)| receipt)
    }

    /// Acknowledge an opening with the disposition of this transaction.
    pub fn egress_open_channel(
        &self,
        request: &EgressChannelOpeningRequestV1,
    ) -> anyhow::Result<EgressChannelOpeningReceiptV1> {
        let (receipt, disposition) = self.record_egress_observation(
            request.version,
            &request.observation_id,
            &EgressObservationV1::ChannelOpened {
                binding: request.binding.clone(),
            },
        )?;
        Ok(EgressChannelOpeningReceiptV1 {
            version: 1,
            observation_id: request.observation_id.clone(),
            binding: request.binding.clone(),
            disposition,
            receipt,
        })
    }

    fn record_egress_observation(
        &self,
        version: u32,
        observation_id: &IdV1,
        observation: &EgressObservationV1,
    ) -> anyhow::Result<(EgressReceiptV1, EgressChannelOpeningDispositionV1)> {
        anyhow::ensure!(version == 1, EgressJournalError::Invalid);
        let kind = observation_kind(observation);
        let binding = binding_of(observation);
        let data = serde_json::to_value(observation)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        if let Some(existing) = by_id(&tx, observation_id)? {
            anyhow::ensure!(
                existing.event.kind == kind && existing.event.data == data,
                EgressJournalError::Conflict
            );
            self.commit_checked(tx)?;
            return Ok((
                receipt(&existing),
                EgressChannelOpeningDispositionV1::Replayed,
            ));
        }
        // Record effects against the historic authorization. A result can arrive
        // after the worker stopped, and resolution can fail before a permit exists.
        if let EgressObservationV1::Connection {
            decision_id,
            flow_id,
            connected,
            failure,
            peer,
            ..
        } = observation
        {
            let stored = by_id(&tx, decision_id)?
                .filter(|event| event.event.kind == DECISION_KIND)
                .ok_or(EgressJournalError::Invalid)?;
            let (decided_request, decision) = stored_decision(&stored)?;
            anyhow::ensure!(
                decided_request.decision_id == *decision_id
                    && decided_request.binding == *binding
                    && decided_request.flow_id == *flow_id
                    && decision.allows(&decided_request),
                EgressJournalError::Invalid
            );
            if *connected {
                anyhow::ensure!(failure.is_none(), EgressJournalError::Invalid);
                let peer = peer.as_deref().ok_or(EgressJournalError::Invalid)?;
                require_connected_permit(&tx, &decided_request, stored.seq, peer)?;
            }
        }
        // A fresh channel opening requires the exact durable Running worker.
        // A replay above still returns the same historical receipt.
        if matches!(observation, EgressObservationV1::ChannelOpened { .. }) {
            anyhow::ensure!(
                worker_facts(&tx, binding)? == WorkerFacts::Eligible,
                EgressJournalError::Invalid
            );
        }
        let event = event(
            kind,
            observation_id,
            &binding.session_id,
            &binding.worker_id,
            data,
        );
        let (event, payload) = self.prepare_egress_event(event)?;
        let stored = self.append_in_transaction(&tx, event, payload)?;
        self.commit_checked(tx)?;
        Ok((receipt(&stored), EgressChannelOpeningDispositionV1::Fresh))
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
        if worker_facts(&tx, &request.binding)? != WorkerFacts::Eligible {
            self.commit_checked(tx)?;
            return Ok(EgressRenewResponseV1 {
                version: 1,
                revoked: request.decisions.clone(),
                expired: Vec::new(),
            });
        }
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
pub(crate) mod tests;
