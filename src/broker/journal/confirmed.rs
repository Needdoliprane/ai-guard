//! Shared verification of confirmed revision bytes. No filesystem or runtime effects.
use super::*;
use crate::broker::lifecycle::LocalCommandRequestV1;

pub(super) struct ConfirmedChainV1 {
    pub payload: LoadedCheckpoint,
    pub legacy_managed_seen: bool,
    pub chain: Vec<RevisionRefV1>,
    pub command_receipts: HashMap<Uuid, LocalCommandReceiptV1>,
    pub cancel_boundaries: HashMap<Uuid, usize>,
    pub outbox_receipts: HashMap<(Uuid, Option<i64>), OutboxReceiptV1>,
    pub last_cancel_operations: Option<usize>,
}

pub(super) struct ChainVerifierV1<'a> {
    scope: &'a JournalScopeV1,
    control: &'a ControlSecrets,
    intent: &'a ExecutionIntentV1,
    intent_hash: String,
    binding: &'a BindingManifestV1,
    initial: &'a RevisionRefV1,
    chain: Vec<RevisionRefV1>,
    payload: Option<LoadedCheckpoint>,
    legacy_managed_seen: bool,
    command_receipts: HashMap<Uuid, LocalCommandReceiptV1>,
    cancel_boundaries: HashMap<Uuid, usize>,
    outbox_receipts: HashMap<(Uuid, Option<i64>), OutboxReceiptV1>,
    last_cancel_operations: Option<usize>,
}
impl<'a> ChainVerifierV1<'a> {
    pub fn new(
        scope: &'a JournalScopeV1,
        control: &'a ControlSecrets,
        intent: &'a ExecutionIntentV1,
        binding: &'a BindingManifestV1,
        initial: &'a RevisionRefV1,
    ) -> JournalResult<Self> {
        Ok(Self {
            scope,
            control,
            intent,
            intent_hash: intent.intent_hash().map_err(|_| JournalError::Corrupt)?,
            binding,
            initial,
            chain: Vec::new(),
            payload: None,
            legacy_managed_seen: false,
            command_receipts: HashMap::new(),
            cancel_boundaries: HashMap::new(),
            outbox_receipts: HashMap::new(),
            last_cancel_operations: None,
        })
    }

    pub fn push(&mut self, revision: u64, bytes: &[u8]) -> JournalResult<()> {
        if revision != self.chain.len() as u64 || bytes.len() as u64 > CHECKPOINT {
            return Err(JournalError::Corrupt);
        }
        self.control.record(bytes)?;
        let previous = self.chain.last().cloned();
        let operation_id = self.intent.operation_id();
        let raw: Value = parse(bytes)?;
        macro_rules! decode {
            ($record:ty, $variant:ident, $domain:literal) => {{
                let checkpoint: $record = parse(bytes)?;
                if checkpoint.operation_id != operation_id
                    || checkpoint.intent_hash != self.intent_hash
                    || checkpoint.revision != revision
                    || checkpoint.previous != previous
                    || canonical(&checkpoint)? != bytes
                {
                    return Err(JournalError::Corrupt);
                }
                (LoadedCheckpoint::$variant(checkpoint.payload), $domain)
            }};
        }
        let (next_payload, domain) = match raw.get("version").and_then(Value::as_u64) {
            Some(1) => {
                let raw_payload = canonical(raw.get("payload").ok_or(JournalError::Corrupt)?)?;
                let checkpoint: Checkpoint = parse(bytes)?;
                CheckpointPayloadV1::from_bytes(&raw_payload)?;
                if checkpoint.operation_id != operation_id
                    || checkpoint.intent_hash != self.intent_hash
                    || checkpoint.revision != revision
                    || checkpoint.previous != previous
                    || canonical(&checkpoint)? != bytes
                {
                    return Err(JournalError::Corrupt);
                }
                (
                    LoadedCheckpoint::V1(checkpoint.payload),
                    "ai-guard:broker-checkpoint:v1",
                )
            }
            Some(2) => decode!(SupervisedCheckpoint, V2, "ai-guard:broker-checkpoint:v2"),
            Some(3) => decode!(LifecycleCheckpoint, V3, "ai-guard:broker-checkpoint:v3"),
            Some(4) => decode!(OutboxCheckpoint, V4, "ai-guard:broker-checkpoint:v4"),
            _ => return Err(JournalError::UnsupportedVersion),
        };
        context_checkpoint(
            self.scope,
            self.control,
            &next_payload,
            self.intent,
            self.binding,
        )?;
        if let Some(old) = &self.payload {
            check_transition(
                old,
                &next_payload,
                self.legacy_managed_seen,
                self.last_cancel_operations,
            )?;
        } else if !matches!(next_payload, LoadedCheckpoint::V1(_)) {
            return Err(JournalError::Corrupt);
        }
        if let LoadedCheckpoint::V1(value) = &next_payload {
            self.legacy_managed_seen |= value.broker_context().managed;
        }
        let head = RevisionRefV1 {
            revision,
            hash: digest(domain, bytes),
        };
        if let Some(value) = next_payload.lifecycle() {
            self.last_cancel_operations = value.cancel_boundary(
                self.payload.as_ref().and_then(LoadedCheckpoint::lifecycle),
                self.last_cancel_operations,
            );
            for command in value.commands() {
                if matches!(command.request(), LocalCommandRequestV1::CancelRequested) {
                    // Operation birth, rather than acknowledgement time, is
                    // the causal boundary. A later Arm ACK must not move it.
                    self.cancel_boundaries
                        .entry(command.command_id())
                        .or_insert(value.broker().supervision().operations().len());
                }
                self.command_receipts
                    .entry(command.command_id())
                    .or_insert_with(|| LocalCommandReceiptV1 {
                        command_id: command.command_id(),
                        input_hash: command.input_hash().into(),
                        head: head.clone(),
                    });
            }
        }
        if let Some(value) = next_payload.outbox() {
            for entry in value.events() {
                self.outbox_receipts
                    .entry((entry.event_id(), entry.audit_seq()))
                    .or_insert_with(|| OutboxReceiptV1 {
                        operation_id,
                        event_id: entry.event_id(),
                        input_hash: entry.input_hash().into(),
                        audit_seq: entry.audit_seq(),
                        head: head.clone(),
                    });
            }
        }
        if revision == 0
            && (*self.initial != head
                || CheckpointPayloadV1::initial(self.intent)?.canonical_bytes()?
                    != next_payload.runner().canonical_bytes()?)
        {
            return Err(JournalError::Corrupt);
        }
        self.chain.push(head);
        self.payload = Some(next_payload);
        Ok(())
    }

    pub fn finish(self, current: &RevisionRefV1) -> JournalResult<ConfirmedChainV1> {
        if self.chain.last() != Some(current) {
            return Err(JournalError::Corrupt);
        }
        Ok(ConfirmedChainV1 {
            payload: self.payload.ok_or(JournalError::Corrupt)?,
            legacy_managed_seen: self.legacy_managed_seen,
            chain: self.chain,
            command_receipts: self.command_receipts,
            cancel_boundaries: self.cancel_boundaries,
            outbox_receipts: self.outbox_receipts,
            last_cancel_operations: self.last_cancel_operations,
        })
    }
}

pub(super) fn context_checkpoint(
    scope: &JournalScopeV1,
    control: &ControlSecrets,
    checkpoint: &LoadedCheckpoint,
    intent: &ExecutionIntentV1,
    binding: &BindingManifestV1,
) -> JournalResult<()> {
    context(scope, control, checkpoint.runner(), intent, binding)?;
    if let Some(value) = checkpoint.supervised() {
        value.validate_context(intent, scope.0.control_namespace_uuid, binding)?;
    }
    if let Some(value) = checkpoint.lifecycle() {
        value.validate_context(
            intent.operation_id(),
            &intent
                .intent_hash()
                .map_err(|_| JournalError::InvalidInput)?,
        )?;
    }
    if let Some(value) = checkpoint.outbox() {
        value.validate_context(intent)?;
    }
    Ok(())
}
pub(super) fn context(
    scope: &JournalScopeV1,
    control: &ControlSecrets,
    checkpoint: &CheckpointPayloadV1,
    intent: &ExecutionIntentV1,
    binding: &BindingManifestV1,
) -> JournalResult<()> {
    let bytes = checkpoint.canonical_bytes()?;
    control.record(&bytes)?;
    let options = intent.initial_options().run_options();
    let expected =
        reqwest::Url::parse(&scope.0.gateway_origin).map_err(|_| JournalError::InvalidInput)?;
    let configured =
        reqwest::Url::parse(&options.gateway).map_err(|_| JournalError::InvalidInput)?;
    if expected.origin() != configured.origin() {
        return Err(JournalError::Conflict);
    }
    checkpoint.validate_journal_context(
        intent,
        &scope.0.tenant_id,
        &scope.0.gateway_origin,
        binding.binding_id(),
        binding.names(),
        binding.handoff_sha(),
    )
}
pub(super) fn check_transition(
    previous: &LoadedCheckpoint,
    next: &LoadedCheckpoint,
    legacy_managed_seen: bool,
    last_cancel_operations: Option<usize>,
) -> JournalResult<()> {
    if let Some(value) = next.outbox() {
        value.validate_transition(previous.outbox())?;
    } else if previous.outbox().is_some() {
        return Err(JournalError::Conflict);
    }
    if let Some(value) = next.lifecycle() {
        if let LoadedCheckpoint::V1(old) = previous
            && (legacy_managed_seen
                || (old.broker_context().request.human_approved
                    && !value
                        .broker()
                        .runner()
                        .broker_context()
                        .request
                        .human_approved))
        {
            return Err(JournalError::Conflict);
        }
        value.broker().validate_transition(previous.supervised())?;
        return value.validate_transition(
            previous.lifecycle(),
            previous.supervised(),
            previous.runner(),
            last_cancel_operations,
        );
    }
    match (previous, next) {
        (LoadedCheckpoint::V1(old), LoadedCheckpoint::V2(value)) => {
            if legacy_managed_seen
                || (old.broker_context().request.human_approved
                    && !value.runner().broker_context().request.human_approved)
            {
                return Err(JournalError::Conflict);
            }
            value.validate_transition(None)
        }
        (LoadedCheckpoint::V2(old), LoadedCheckpoint::V2(value)) => {
            value.validate_transition(Some(old))
        }
        (LoadedCheckpoint::V2(_), LoadedCheckpoint::V1(_)) => Err(JournalError::Conflict),
        (LoadedCheckpoint::V1(_), LoadedCheckpoint::V1(_)) => Ok(()),
        (LoadedCheckpoint::V3(_), _)
        | (_, LoadedCheckpoint::V3(_))
        | (LoadedCheckpoint::V4(_), _)
        | (_, LoadedCheckpoint::V4(_)) => Err(JournalError::Conflict),
    }
}
