//! Durable event delivery data. Neither an event nor its delivery ACK grants authority.
use super::{
    checkpoint::SupervisionOperationV1,
    intent::ExecutionIntentV1,
    journal::{JournalError, JournalResult, RevisionRefV1, canonical, digest, valid_hash},
    lifecycle::BrokerCheckpointV3,
};
use crate::model::{CommandSpec, Decision, Event, Outcome};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;
use std::{collections::HashSet, fmt};
use uuid::Uuid;

const MAX_EVENTS: usize = 512;
const MAX_EVENT_BYTES: usize = 120 * 1024;
const MAX_OUTBOX_BYTES: usize = 4 * 1024 * 1024;
const MAX_CHECKPOINT_BYTES: usize = 32 * 1024 * 1024;

fn require(valid: bool) -> JournalResult<()> {
    if valid {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}

/// IPC command identities span local commands, explicit resumes (events) and
/// export requests. Validate them in the same revision, so racing requests
/// cannot bypass a service pre-check by entering different persistent lists.
pub(crate) fn validate_command_ids(
    lifecycle: &BrokerCheckpointV3,
    events: &[OutboxEventV1],
) -> JournalResult<()> {
    let mut ids = HashSet::new();
    for command in lifecycle.commands() {
        require(ids.insert(command.command_id()))?;
    }
    for event in events {
        require(ids.insert(event.event_id()))?;
    }
    for operation in lifecycle.broker().supervision().operations() {
        if let SupervisionOperationV1::Export { request, .. } = operation {
            let id =
                Uuid::parse_str(&request.operation_id).map_err(|_| JournalError::InvalidInput)?;
            require(ids.insert(id))?;
        }
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxEventV1 {
    version: u32,
    event: Event,
    #[serde(skip)]
    event_id: Uuid,
    input_hash: String,
    audit_seq: Option<i64>,
}

// Event::Decision is historically permissive. Close it here, including when
// an enclosing tagged enum has buffered the JSON before deserializing us.
fn deserialize_event<'de, D: Deserializer<'de>>(decoder: D) -> Result<Event, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DecisionWire {
        outcome: Outcome,
        engine: String,
        rule: String,
        reason: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EventWire {
        id: String,
        timestamp: String,
        kind: String,
        source: String,
        provenance: String,
        coverage: String,
        session_id: Option<String>,
        project_id: Option<String>,
        sandbox_id: Option<String>,
        agent_id: Option<String>,
        parent_agent_id: Option<String>,
        correlation_id: Option<String>,
        trace_id: Option<String>,
        command: Option<CommandSpec>,
        decision: Option<DecisionWire>,
        result: Option<Value>,
        data: Value,
    }
    let wire = EventWire::deserialize(decoder)
        .map_err(|_| D::Error::custom("invalid private outbox event"))?;
    Ok(Event {
        id: wire.id,
        timestamp: wire.timestamp,
        kind: wire.kind,
        source: wire.source,
        provenance: wire.provenance,
        coverage: wire.coverage,
        session_id: wire.session_id,
        project_id: wire.project_id,
        sandbox_id: wire.sandbox_id,
        agent_id: wire.agent_id,
        parent_agent_id: wire.parent_agent_id,
        correlation_id: wire.correlation_id,
        trace_id: wire.trace_id,
        command: wire.command,
        decision: wire.decision.map(|v| Decision {
            outcome: v.outcome,
            engine: v.engine,
            rule: v.rule,
            reason: v.reason,
        }),
        result: wire.result,
        data: wire.data,
    })
}

impl<'de> Deserialize<'de> for OutboxEventV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            #[serde(deserialize_with = "deserialize_event")]
            event: Event,
            input_hash: String,
            audit_seq: Option<i64>,
        }
        let wire = Wire::deserialize(decoder)
            .map_err(|_| D::Error::custom("invalid private outbox entry"))?;
        let value = Self {
            version: wire.version,
            event_id: Uuid::parse_str(&wire.event.id)
                .map_err(|_| D::Error::custom("invalid private outbox entry"))?,
            event: wire.event,
            input_hash: wire.input_hash,
            audit_seq: wire.audit_seq,
        };
        value
            .validate_shape()
            .map_err(|_| D::Error::custom("invalid private outbox entry"))?;
        Ok(value)
    }
}

impl OutboxEventV1 {
    pub fn new(operation_id: Uuid, intent_hash: &str, event: Event) -> JournalResult<Self> {
        let mut value = Self {
            version: 1,
            event_id: Uuid::parse_str(&event.id).map_err(|_| JournalError::InvalidInput)?,
            event,
            input_hash: String::new(),
            audit_seq: None,
        };
        value.input_hash = value.context_hash(operation_id, intent_hash)?;
        value.validate_shape()?;
        Ok(value)
    }
    pub fn event(&self) -> &Event {
        &self.event
    }
    pub fn event_id(&self) -> Uuid {
        self.event_id
    }
    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }
    pub fn audit_seq(&self) -> Option<i64> {
        self.audit_seq
    }
    pub fn acknowledged(&self, audit_seq: i64) -> JournalResult<Self> {
        if audit_seq <= 0 {
            return Err(JournalError::InvalidInput);
        }
        require(self.audit_seq.is_none_or(|old| old == audit_seq))?;
        let mut value = self.clone();
        value.audit_seq = Some(audit_seq);
        Ok(value)
    }
    fn context_hash(&self, operation_id: Uuid, intent_hash: &str) -> JournalResult<String> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        valid_hash(intent_hash)?;
        #[derive(Serialize)]
        struct Input<'a> {
            version: u32,
            operation_id: Uuid,
            intent_hash: &'a str,
            event: &'a Event,
        }
        Ok(digest(
            "ai-guard:broker-outbox-event:v1",
            &canonical(&Input {
                version: 1,
                operation_id,
                intent_hash,
                event: &self.event,
            })?,
        ))
    }
    fn validate_shape(&self) -> JournalResult<()> {
        if self.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        valid_hash(&self.input_hash)?;
        self.event
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        let id = Uuid::parse_str(&self.event.id).map_err(|_| JournalError::InvalidInput)?;
        if id.is_nil()
            || id != self.event_id
            || id.to_string() != self.event.id
            || self.event.timestamp.len() > 64
            || self.event.source.is_empty()
            || self.event.source.chars().any(char::is_control)
            || self.audit_seq.is_some_and(|seq| seq <= 0)
            || ["security.decision", "control", "worker", "usage.allocation"]
                .iter()
                .any(|namespace| {
                    self.event.kind == *namespace
                        || self
                            .event
                            .kind
                            .strip_prefix(namespace)
                            .is_some_and(|suffix| suffix.starts_with('.'))
                })
        {
            return Err(JournalError::InvalidInput);
        }
        if canonical(&self.event)?.len() > MAX_EVENT_BYTES {
            return Err(JournalError::TooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointV4 {
    version: u32,
    lifecycle: BrokerCheckpointV3,
    events: Vec<OutboxEventV1>,
}
impl<'de> Deserialize<'de> for BrokerCheckpointV4 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            lifecycle: BrokerCheckpointV3,
            events: Vec<OutboxEventV1>,
        }
        let mut ignored = false;
        let wire: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
            .map_err(|_| D::Error::custom("invalid private outbox checkpoint"))?;
        if ignored {
            return Err(D::Error::custom("invalid private outbox checkpoint"));
        }
        let value = Self {
            version: wire.version,
            lifecycle: wire.lifecycle,
            events: wire.events,
        };
        value
            .validate_shape()
            .map_err(|_| D::Error::custom("invalid private outbox checkpoint"))?;
        Ok(value)
    }
}
impl BrokerCheckpointV4 {
    pub fn new(lifecycle: BrokerCheckpointV3, events: Vec<OutboxEventV1>) -> JournalResult<Self> {
        let value = Self {
            version: 4,
            lifecycle,
            events,
        };
        value.validate_shape()?;
        Ok(value)
    }
    pub fn lifecycle(&self) -> &BrokerCheckpointV3 {
        &self.lifecycle
    }
    pub fn events(&self) -> &[OutboxEventV1] {
        &self.events
    }
    pub fn from_bytes(bytes: &[u8]) -> JournalResult<Self> {
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(JournalError::TooLarge);
        }
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let value = Self::deserialize(&mut decoder).map_err(|_| JournalError::InvalidInput)?;
        decoder.end().map_err(|_| JournalError::InvalidInput)?;
        Ok(value)
    }
    pub fn canonical_bytes(&self) -> JournalResult<Vec<u8>> {
        self.validated_canonical_bytes()
    }
    fn validate_shape(&self) -> JournalResult<()> {
        self.validated_canonical_bytes().map(drop)
    }
    fn validated_canonical_bytes(&self) -> JournalResult<Vec<u8>> {
        if self.version != 4 {
            return Err(JournalError::UnsupportedVersion);
        }
        if self.events.len() > MAX_EVENTS {
            return Err(JournalError::LimitExceeded);
        }
        self.lifecycle.canonical_bytes()?;
        for entry in &self.events {
            entry.validate_shape()?;
        }
        validate_command_ids(&self.lifecycle, &self.events)?;
        if canonical(&self.events)?.len() > MAX_OUTBOX_BYTES {
            return Err(JournalError::TooLarge);
        }
        let bytes = canonical(self)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(JournalError::TooLarge);
        }
        Ok(bytes)
    }
    pub(crate) fn validate_context(&self, intent: &ExecutionIntentV1) -> JournalResult<()> {
        self.validate_shape()?;
        let request = intent.request_data();
        let intent_hash = intent
            .intent_hash()
            .map_err(|_| JournalError::InvalidInput)?;
        for entry in &self.events {
            let event = entry.event();
            require(entry.input_hash == entry.context_hash(intent.operation_id(), &intent_hash)?)?;
            require(
                event.session_id == request.session_id
                    && event.project_id == request.project_id
                    && event.sandbox_id == request.sandbox_id
                    && event.correlation_id == request.correlation_id
                    && event.trace_id == request.trace_id,
            )?;
        }
        Ok(())
    }
    pub(crate) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        self.validate_shape()?;
        let old = previous.map_or(&[][..], |value| value.events());
        require(self.events.len() >= old.len())?;
        for (index, entry) in self.events.iter().enumerate() {
            if let Some(previous) = old.get(index) {
                require(
                    entry.input_hash == previous.input_hash
                        && canonical(&entry.event)? == canonical(&previous.event)?,
                )?;
                if let Some(seq) = previous.audit_seq {
                    require(entry.audit_seq == Some(seq))?;
                }
            } else {
                // An ACK can only be added after this entry's pending revision committed.
                require(entry.audit_seq.is_none())?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct OutboxReceiptV1 {
    pub(crate) operation_id: Uuid,
    pub(crate) event_id: Uuid,
    pub(crate) input_hash: String,
    pub(crate) audit_seq: Option<i64>,
    pub(crate) head: RevisionRefV1,
}
impl OutboxReceiptV1 {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn event_id(&self) -> Uuid {
        self.event_id
    }
    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }
    pub fn audit_seq(&self) -> Option<i64> {
        self.audit_seq
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
}
macro_rules! opaque {
    ($($kind:ty),+ $(,)?) => {$(impl fmt::Debug for $kind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(concat!(stringify!($kind), "(<private>)"))
        }
    })+};
}
opaque!(OutboxEventV1, BrokerCheckpointV4, OutboxReceiptV1);
