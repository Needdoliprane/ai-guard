//! Tenant-local typed journal in existing events. Projection is rebuilt under
//! the same checked SQLite transaction; no memory cache grants channel authority.
use super::*;
use crate::{
    control::{ArmSubjectV1, ArmV1, RuntimeStatusV1, WorkerStateV1, WorkerV1},
    relay::admission::*,
    workload::{WorkloadBinding, WorkloadSubjectV1},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Input {
    Reserve(Box<RelayChannelAdmissionV1>),
    Claim(RelayChannelClaimV1),
    Transition(RelayChannelTransitionV1),
}
impl Input {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Reserve(r) => r.validate(),
            Self::Claim(r) => r.validate(),
            Self::Transition(r) => r.validate(),
        }
    }
    fn operation_id(&self) -> &str {
        match self {
            Self::Reserve(r) => &r.operation_id,
            Self::Claim(r) => &r.operation_id,
            Self::Transition(r) => &r.operation_id,
        }
    }
    fn channel_id(&self) -> &str {
        match self {
            Self::Reserve(r) => &r.channel_id,
            Self::Claim(r) => &r.channel_id,
            Self::Transition(r) => &r.channel_id,
        }
    }
    fn epoch(&self) -> u64 {
        match self {
            Self::Reserve(r) => r.epoch,
            Self::Claim(r) => r.epoch,
            Self::Transition(r) => r.epoch,
        }
    }
    fn input_hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Reserve(r) => r.input_hash(),
            Self::Claim(r) => r.input_hash(),
            Self::Transition(r) => r.input_hash(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Initiator {
    LegacyRunner,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    tenant_id: String,
    initiator: Initiator,
    input: Input,
    input_hash: String,
}
struct Channel {
    admission: RelayChannelAdmissionV1,
    snapshot: RelayChannelSnapshotV1,
}
#[derive(Default)]
struct Projection {
    channels: BTreeMap<String, Channel>,
    operations: BTreeMap<String, (String, RelayChannelReceiptV1)>,
    latest_endpoint: BTreeMap<String, String>,
    count: usize,
}
fn conflict(value: bool) -> anyhow::Result<()> {
    require(value, RelayAdmissionFailure::Conflict)
}
fn allowed(value: bool) -> anyhow::Result<()> {
    require(value, RelayAdmissionFailure::Forbidden)
}
fn capacity(value: bool) -> anyhow::Result<()> {
    require(value, RelayAdmissionFailure::Capacity)
}
fn endpoint_key(input: &RelayChannelAdmissionV1) -> anyhow::Result<String> {
    crate::control::content_hash("ai-guard:relay-channel:endpoint:v1", &input.endpoint)
}
fn namespace(db: &Connection) -> anyhow::Result<String> {
    db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )
    .map_err(Into::into)
}
fn worker(db: &Connection, id: &str, before: Option<i64>) -> anyhow::Result<Option<WorkerV1>> {
    if let Some(seq) = before {
        return super::control::worker_at(db, id, seq);
    }
    db.query_row(
        "SELECT snapshot_json FROM workers WHERE worker_id=?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}

/// Check stored control facts at a historical prefix or at the current snapshot.
/// Reported runtime facts are never upgraded to independent gateway observations.
fn authority(
    db: &Connection,
    input: &RelayChannelAdmissionV1,
    at: DateTime<Utc>,
    before: Option<i64>,
    exact_revision: bool,
) -> anyhow::Result<()> {
    super::workload::validate_control_time_floor(db, at, before)?;
    allowed(namespace(db)? == input.control_namespace_uuid)?;
    let binding: WorkloadBinding = if let Some(seq) = before {
        super::workload::prefix_binding(db, &input.credential_id, seq)?
    } else {
        db.query_row(
            "SELECT payload_json FROM workload_bindings WHERE credential_id=?1",
            [&input.credential_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|json| serde_json::from_str(&json))
        .transpose()?
    }
    .ok_or(RelayAdmissionFailure::Forbidden)?;
    allowed(
        !binding.revoked
            && binding.credential_id == input.credential_id
            && binding.revision == input.binding_revision
            && binding.sha256_digest == input.credential_sha256_digest
            && binding.subject
                == (WorkloadSubjectV1::Worker {
                    worker_id: input.endpoint.worker_id.clone(),
                    attempt_id: input.endpoint.attempt_id.clone(),
                    generation: input.endpoint.generation,
                })
            && at >= time(&binding.updated_at)?
            && at < time(&binding.expires_at)?,
    )?;
    let worker =
        worker(db, &input.endpoint.worker_id, before)?.ok_or(RelayAdmissionFailure::Forbidden)?;
    let observed = worker
        .last_observation
        .as_ref()
        .ok_or(RelayAdmissionFailure::Forbidden)?;
    allowed(
        worker.worker_id == input.endpoint.worker_id
            && worker.attempt.attempt_id == input.endpoint.attempt_id
            && input.endpoint.generation == 1
            && worker.quota_held
            && !worker.allocation_inconsistent
            && worker.state == WorkerStateV1::Running
            && worker.launch.session_id == input.session_id
            && (if exact_revision {
                worker.revision == input.expected_worker_revision
            } else {
                worker.revision >= input.expected_worker_revision
            })
            && worker.attempt.container_id.as_deref()
                == Some(input.inspection.container_id.as_str())
            && worker.attempt.created_inspection.as_ref() == Some(&input.inspection)
            && observed.inspection == input.inspection
            && observed.status == RuntimeStatusV1::Running
            && observed.running == Some(true)
            && observed.restart_count == Some(0)
            && worker.launch.network == crate::control::NetworkV1::None,
    )?;
    let started = time(&input.started_at)?;
    allowed(
        worker
            .observed_started_at
            .as_deref()
            .map(time)
            .transpose()?
            == Some(started)
            && observed.started_at.as_deref().map(time).transpose()? == Some(started)
            && started <= at
            && time(&observed.observed_at)? <= at,
    )?;
    let mut statement = db.prepare("SELECT payload_json FROM execution_arms WHERE worker_id=?1 AND attempt_id=?2 AND event_seq<?3 LIMIT 65")?;
    let rows = statement
        .query_map(
            params![
                input.endpoint.worker_id,
                input.endpoint.attempt_id,
                before.unwrap_or(i64::MAX)
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    capacity(rows.len() <= 64)?;
    let mut arm = None;
    for json in rows {
        let entry: ArmV1 = serde_json::from_str(&json)?;
        if matches!(entry.subject, ArmSubjectV1::Worker) {
            conflict(arm.is_none())?;
            arm = Some(entry);
        }
    }
    let arm = arm.ok_or(RelayAdmissionFailure::Forbidden)?;
    allowed(
        arm.worker_id == input.endpoint.worker_id
            && arm.attempt_id == input.endpoint.attempt_id
            && arm.inspection == input.inspection
            && arm.authority_binding_hash == input.authority_binding_hash
            && arm.configuration_fingerprint_version == input.configuration_fingerprint_version
            && arm.configuration_fingerprint == input.configuration_fingerprint,
    )?;
    let arm_utc = time(
        arm.clock
            .observed_utc
            .as_deref()
            .ok_or(RelayAdmissionFailure::Forbidden)?,
    )?;
    let expiry = time(&input.expires_at)?;
    let worker_cap = started
        .checked_add_signed(Duration::seconds(i64::from(worker.launch.timeout_seconds)))
        .ok_or(RelayAdmissionFailure::InvalidRequest)?;
    let arm_cap = arm_utc
        .checked_add_signed(Duration::seconds(i64::from(arm.timeout_seconds)))
        .ok_or(RelayAdmissionFailure::InvalidRequest)?;
    allowed(
        at < expiry
            && expiry <= time(&binding.expires_at)?
            && expiry <= worker_cap
            && expiry <= arm_cap,
    )
}

fn terminal(
    db: &Connection,
    admission: &RelayChannelAdmissionV1,
    revision: u64,
    observation_id: &str,
    before: Option<i64>,
) -> anyhow::Result<()> {
    let current = worker(db, &admission.endpoint.worker_id, before)?
        .ok_or(RelayAdmissionFailure::Conflict)?;
    conflict(
        current.state == WorkerStateV1::Terminal
            && !current.quota_held
            && !current.allocation_inconsistent
            && current.revision == revision
            && current.attempt.attempt_id == admission.endpoint.attempt_id
            && current.attempt.created_inspection.as_ref() == Some(&admission.inspection)
            && current
                .observed_started_at
                .as_deref()
                .map(time)
                .transpose()?
                == Some(time(&admission.started_at)?),
    )?;
    let persisted: Option<(String, String, String, i64)> = db.query_row(
        "SELECT a.terminal_observation_id,o.worker_id,o.payload_json,o.event_seq FROM worker_allocations a JOIN worker_observations o ON o.observation_id=a.terminal_observation_id WHERE a.attempt_id=?1",
        [&admission.endpoint.attempt_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (id, worker_id, json, seq) = persisted.ok_or(RelayAdmissionFailure::Conflict)?;
    let observation: crate::control::RuntimeObservationV1 = serde_json::from_str(&json)?;
    conflict(
        id == observation_id
            && worker_id == admission.endpoint.worker_id
            && seq < before.unwrap_or(i64::MAX)
            && observation.inspection == admission.inspection
            && observation.running == Some(false)
            && matches!(
                observation.status,
                RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
            )
            && observation.restart_count == Some(0)
            && observation.started_at.as_deref().map(time).transpose()?
                == Some(time(&admission.started_at)?),
    )
}

impl Projection {
    fn apply(
        &mut self,
        db: &Connection,
        record: &Record,
        at: DateTime<Utc>,
        seq: i64,
        historical: bool,
    ) -> anyhow::Result<()> {
        record.input.validate()?;
        conflict(
            record.version == 1
                && record.initiator == Initiator::LegacyRunner
                && record.input_hash == record.input.input_hash()?
                && !self.operations.contains_key(record.input.operation_id()),
        )?;
        let before = historical.then_some(seq);
        match &record.input {
            Input::Reserve(input) => {
                authority(db, input, at, before, true)?;
                conflict(!self.channels.contains_key(&input.channel_id))?;
                let key = endpoint_key(input)?;
                if let Some(previous) = self.latest_endpoint.get(&key) {
                    let previous = self
                        .channels
                        .get(previous)
                        .context("relay endpoint projection missing")?;
                    conflict(
                        !previous.snapshot.state.active()
                            && previous.snapshot.epoch.checked_add(1) == Some(input.epoch),
                    )?;
                } else {
                    conflict(input.epoch == 1)?;
                }
                capacity(
                    self.channels
                        .values()
                        .filter(|c| c.snapshot.state.active())
                        .count()
                        < MAX_ACTIVE_CHANNELS,
                )?;
                self.latest_endpoint.insert(key, input.channel_id.clone());
                let snapshot = RelayChannelSnapshotV1 {
                    version: 1,
                    tenant_id: record.tenant_id.clone(),
                    control_namespace_uuid: input.control_namespace_uuid.clone(),
                    channel_id: input.channel_id.clone(),
                    epoch: input.epoch,
                    admission_operation_id: input.operation_id.clone(),
                    admission_input_hash: record.input_hash.clone(),
                    endpoint: input.endpoint.clone(),
                    credential_id: input.credential_id.clone(),
                    binding_revision: input.binding_revision,
                    session_id: input.session_id.clone(),
                    expires_at: input.expires_at.clone(),
                    revision: 1,
                    state: RelayChannelStateV1::Reserved,
                };
                self.channels.insert(
                    input.channel_id.clone(),
                    Channel {
                        admission: (**input).clone(),
                        snapshot,
                    },
                );
            }
            Input::Claim(input) => {
                let channel = self
                    .channels
                    .get_mut(&input.channel_id)
                    .ok_or(RelayAdmissionFailure::NotFound)?;
                identity(
                    &channel.snapshot,
                    input.epoch,
                    input.expected_revision,
                    &input.admission_input_hash,
                )?;
                conflict(channel.snapshot.state == RelayChannelStateV1::Reserved)?;
                authority(db, &channel.admission, at, before, false)?;
                channel.snapshot.state = RelayChannelStateV1::AcceptedBeforeExec;
                channel.snapshot.revision += 1;
            }
            Input::Transition(input) => {
                let channel = self
                    .channels
                    .get_mut(&input.channel_id)
                    .ok_or(RelayAdmissionFailure::NotFound)?;
                identity(
                    &channel.snapshot,
                    input.epoch,
                    input.expected_revision,
                    &input.admission_input_hash,
                )?;
                use RelayChannelStateV1 as S;
                use RelayChannelTransitionKindV1 as T;
                let state = channel.snapshot.state;
                channel.snapshot.state = match &input.transition {
                    T::CancelReserved => {
                        conflict(state == S::Reserved)?;
                        S::CancelledBeforeClaim
                    }
                    T::KnownRefusedBeforeSpawn => {
                        conflict(state == S::Reserved)?;
                        S::KnownRefusedBeforeSpawn
                    }
                    T::Ready => {
                        conflict(state == S::AcceptedBeforeExec)?;
                        authority(db, &channel.admission, at, before, false)?;
                        S::Ready
                    }
                    T::Closing => {
                        conflict(matches!(state, S::AcceptedBeforeExec | S::Ready))?;
                        S::Closing
                    }
                    T::Unknown => {
                        conflict(matches!(
                            state,
                            S::AcceptedBeforeExec | S::Ready | S::Closing
                        ))?;
                        S::Unknown
                    }
                    T::CloseExactWorkerTerminal {
                        worker_revision,
                        terminal_observation_id,
                    } => {
                        conflict(matches!(
                            state,
                            S::AcceptedBeforeExec | S::Ready | S::Closing | S::Unknown
                        ))?;
                        terminal(
                            db,
                            &channel.admission,
                            *worker_revision,
                            terminal_observation_id,
                            before,
                        )?;
                        S::Closed
                    }
                };
                channel.snapshot.revision += 1;
            }
        }
        self.count += 1;
        capacity(self.count <= MAX_CHANNEL_FACTS)
    }
}
fn identity(
    snapshot: &RelayChannelSnapshotV1,
    epoch: u64,
    revision: u64,
    hash: &str,
) -> anyhow::Result<()> {
    conflict(
        snapshot.epoch == epoch
            && snapshot.revision == revision
            && snapshot.admission_input_hash == hash,
    )
}

fn reconstruct(db: &Connection) -> anyhow::Result<Projection> {
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind='relay_channel' OR kind GLOB 'relay_channel.*' ORDER BY seq LIMIT 4097")?;
    let rows = statement
        .query_map([], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    capacity(rows.len() <= MAX_CHANNEL_FACTS)?;
    if rows.is_empty() {
        return Ok(Projection::default());
    }
    let schema: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    allowed([6, 7, 8, 9, 10, 11].contains(&schema))?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut projection = Projection::default();
    for stored in rows {
        let event = &stored.event;
        conflict(
            event.kind == CHANNEL_EVENT_KIND
                && event.source == "gateway"
                && event.provenance == "observed"
                && event.coverage == "observed"
                && event.command.is_none()
                && event.decision.is_none()
                && event.result.is_none()
                && event.agent_id.is_none()
                && event.parent_agent_id.is_none(),
        )?;
        let record: Record = serde_json::from_value(event.data.clone())?;
        conflict(record.tenant_id == tenant)?;
        let at = time(&event.timestamp)?;
        allowed(time(&stored.received_at)? >= at)?;
        super::workload::validate_control_time_floor(db, at, Some(stored.seq))?;
        projection.apply(db, &record, at, stored.seq, true)?;
        let receipt = RelayChannelReceiptV1 {
            version: 1,
            operation_id: record.input.operation_id().into(),
            channel_id: record.input.channel_id().into(),
            epoch: record.input.epoch(),
            input_hash: record.input_hash.clone(),
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        };
        projection.operations.insert(
            record.input.operation_id().into(),
            (record.input_hash, receipt),
        );
    }
    Ok(projection)
}
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    reconstruct(db).map(|_| ())
}

impl Store {
    pub fn reserve_relay_channel(
        &self,
        request: &RelayChannelAdmissionV1,
    ) -> anyhow::Result<RelayChannelResultV1> {
        self.persist_relay_channel(Input::Reserve(Box::new(request.clone())))
    }
    pub fn claim_relay_channel_exec(
        &self,
        request: &RelayChannelClaimV1,
    ) -> anyhow::Result<RelayChannelResultV1> {
        self.persist_relay_channel(Input::Claim(request.clone()))
    }
    pub fn transition_relay_channel(
        &self,
        request: &RelayChannelTransitionV1,
    ) -> anyhow::Result<RelayChannelResultV1> {
        self.persist_relay_channel(Input::Transition(request.clone()))
    }
    fn persist_relay_channel(&self, input: Input) -> anyhow::Result<RelayChannelResultV1> {
        input.validate()?;
        let record = Record {
            version: 1,
            tenant_id: self.tenant_id.clone(),
            initiator: Initiator::LegacyRunner,
            input_hash: input.input_hash()?,
            input,
        };
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let schema: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if ![6, 7, 8, 9, 10, 11].contains(&schema) {
            return Err(crate::control::ControlFailure::MigrationRequired.into());
        }
        let clock = super::workload::capture_control_time(&tx)?;
        let mut projection = reconstruct(&tx)?;
        if let Some((hash, receipt)) = projection.operations.get(record.input.operation_id()) {
            conflict(
                hash == &record.input_hash && receipt.channel_id == record.input.channel_id(),
            )?;
            let channel = projection
                .channels
                .get(record.input.channel_id())
                .ok_or(RelayAdmissionFailure::Conflict)?;
            if matches!(&record.input, Input::Reserve(_) | Input::Claim(_)) {
                allowed(tx.lease.active)?;
                authority(&tx, &channel.admission, clock.at(), None, false)?;
                self.checked_worker_relay_endpoint(&tx, &channel.admission.endpoint, clock.at())?;
            }
            let response = RelayChannelResultV1 {
                version: 1,
                replayed: true,
                channel: channel.snapshot.clone(),
                receipt: receipt.clone(),
            };
            self.commit_authorized(tx, &clock)?;
            return Ok(response);
        }
        if matches!(&record.input, Input::Reserve(_) | Input::Claim(_))
            || matches!(&record.input, Input::Transition(r) if matches!(r.transition, RelayChannelTransitionKindV1::Ready))
        {
            allowed(tx.lease.active)?;
            let endpoint = match &record.input {
                Input::Reserve(r) => &r.endpoint,
                _ => {
                    &projection
                        .channels
                        .get(record.input.channel_id())
                        .ok_or(RelayAdmissionFailure::NotFound)?
                        .admission
                        .endpoint
                }
            };
            self.checked_worker_relay_endpoint(&tx, endpoint, clock.at())?;
        }
        capacity(projection.count < MAX_CHANNEL_FACTS)?;
        projection.apply(&tx, &record, clock.at(), i64::MAX, false)?;
        let mut event = Event::new(CHANNEL_EVENT_KIND);
        event.timestamp = clock.at().to_rfc3339_opts(SecondsFormat::Millis, true);
        event.data = serde_json::to_value(&record)?;
        let raw = serde_json::to_value(&event)?;
        require(
            self.redactor.value(&raw) == raw,
            RelayAdmissionFailure::InvalidRequest,
        )?;
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_authorized_in_transaction(&tx, event, payload, &clock)?;
        let channel = projection
            .channels
            .get(record.input.channel_id())
            .context("relay channel write missing")?
            .snapshot
            .clone();
        let receipt = RelayChannelReceiptV1 {
            version: 1,
            operation_id: record.input.operation_id().into(),
            channel_id: record.input.channel_id().into(),
            epoch: record.input.epoch(),
            input_hash: record.input_hash,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        };
        self.commit_authorized(tx, &clock)?;
        Ok(RelayChannelResultV1 {
            version: 1,
            replayed: false,
            channel,
            receipt,
        })
    }
}
