//! Shared verification of confirmed revision bytes. No filesystem or runtime effects.
use super::*;
use crate::{
    broker::{checkpoint::SupervisionOperationV1, lifecycle::LocalCommandRequestV1},
    control::WorkerTransitionV1,
};

#[derive(Clone)]
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
    launch: Option<&'a DockerLaunchProfileBindingV1>,
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
    #[cfg(test)]
    pub fn new(
        scope: &'a JournalScopeV1,
        control: &'a ControlSecrets,
        intent: &'a ExecutionIntentV1,
        binding: &'a BindingManifestV1,
        initial: &'a RevisionRefV1,
    ) -> JournalResult<Self> {
        Self::new_with_launch_profile(scope, control, intent, binding, initial, None)
    }
    pub fn new_with_launch_profile(
        scope: &'a JournalScopeV1,
        control: &'a ControlSecrets,
        intent: &'a ExecutionIntentV1,
        binding: &'a BindingManifestV1,
        initial: &'a RevisionRefV1,
        launch: Option<&'a DockerLaunchProfileBindingV1>,
    ) -> JournalResult<Self> {
        Ok(Self {
            scope,
            control,
            intent,
            intent_hash: intent.intent_hash().map_err(|_| JournalError::Corrupt)?,
            binding,
            launch,
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

    /// Resume only after the caller has rechecked every file in this prefix
    /// under the same operation, intent and binding.
    pub(super) fn with_verified_prefix(mut self, prefix: ConfirmedChainV1) -> Self {
        self.chain = prefix.chain;
        self.payload = Some(prefix.payload);
        self.legacy_managed_seen = prefix.legacy_managed_seen;
        self.command_receipts = prefix.command_receipts;
        self.cancel_boundaries = prefix.cancel_boundaries;
        self.outbox_receipts = prefix.outbox_receipts;
        self.last_cancel_operations = prefix.last_cancel_operations;
        self
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
            Some(5) => decode!(RetirementCheckpoint, V5, "ai-guard:broker-checkpoint:v5"),
            _ => return Err(JournalError::UnsupportedVersion),
        };
        context_checkpoint_with_launch(
            self.scope,
            self.control,
            &next_payload,
            self.intent,
            self.binding,
            self.launch,
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

pub(super) fn context_checkpoint_with_launch(
    scope: &JournalScopeV1,
    control: &ControlSecrets,
    checkpoint: &LoadedCheckpoint,
    intent: &ExecutionIntentV1,
    binding: &BindingManifestV1,
    launch: Option<&DockerLaunchProfileBindingV1>,
) -> JournalResult<()> {
    context(scope, control, checkpoint.runner(), intent, binding)?;
    if let Some(profile) = launch {
        profile.validate().map_err(|_| JournalError::Conflict)?;
        if profile.journal_scope_hash
            != digest("ai-guard:broker-journal-scope:v1", &canonical(scope)?)
            || profile.parent_operation_id != intent.operation_id()
            || profile.intent_hash != intent.intent_hash().map_err(|_| JournalError::Conflict)?
        {
            return Err(JournalError::Conflict);
        }
    }
    let context = checkpoint.runner().broker_context();
    match context.binding {
        Some(authority) => {
            if authority.docker_launch_profile.as_ref() != launch {
                return Err(JournalError::Conflict);
            }
            authority
                .validate_docker_launch_profile()
                .map_err(|_| JournalError::Conflict)?;
        }
        None if launch.is_some() && context.managed => return Err(JournalError::Conflict),
        None => {}
    }
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
    if let Some(value) = checkpoint.retirement() {
        value.validate_context(
            scope,
            intent.operation_id(),
            &intent.intent_hash().map_err(|_| JournalError::Conflict)?,
        )?;
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
    validate_egress_release_transition(previous, next)?;
    if let Some(profile) = previous
        .runner()
        .broker_context()
        .binding
        .and_then(|authority| authority.docker_launch_profile.as_ref())
        && next
            .runner()
            .broker_context()
            .binding
            .and_then(|authority| authority.docker_launch_profile.as_ref())
            != Some(profile)
    {
        // Once a strict authority exists, removing the whole managed projection
        // must not erase its engaged start profile. Legacy transitions stay exact.
        return Err(JournalError::Conflict);
    }
    if let Some(value) = next.retirement() {
        value.validate_transition(previous.retirement())?;
    } else if previous.retirement().is_some() {
        return Err(JournalError::Conflict);
    }
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
        | (_, LoadedCheckpoint::V4(_))
        | (LoadedCheckpoint::V5(_), _)
        | (_, LoadedCheckpoint::V5(_)) => Err(JournalError::Conflict),
    }
}

/// A release intent may be first recorded only on an already acknowledged
/// Running attempt with no durable STOP. Later revisions preserve it byte for
/// byte, even after the worker stops or its terminal audit is reconciled.
fn validate_egress_release_transition(
    previous: &LoadedCheckpoint,
    next: &LoadedCheckpoint,
) -> JournalResult<()> {
    let old = previous.runner().broker_context();
    let new = next.runner().broker_context();
    if old.egress_release == new.egress_release {
        return Ok(());
    }
    let (None, Some(release), Some(previous_lifecycle), Some(lifecycle)) = (
        old.egress_release,
        new.egress_release,
        previous.lifecycle(),
        next.lifecycle(),
    ) else {
        return Err(JournalError::Conflict);
    };
    let previous_supervision = previous_lifecycle.broker().supervision();
    let current_supervision = lifecycle.broker().supervision();
    // This append records only intent. In particular it cannot also acquit
    // a worker mutation, including one that supersedes the Running receipt.
    if canonical(previous_supervision)? != canonical(current_supervision)? {
        return Err(JournalError::Conflict);
    }
    let latest_worker_ack = previous_supervision.operations().iter().rfind(|operation| {
        matches!(
            operation,
            SupervisionOperationV1::WorkerMutation {
                response: Some(_),
                ..
            } | SupervisionOperationV1::WorkerClaim {
                response: Some(_),
                ..
            }
        )
    });
    if matches!(
        latest_worker_ack,
        Some(SupervisionOperationV1::WorkerMutation { request, response: Some(response) })
            if matches!(&request.transition, WorkerTransitionV1::Observe { .. })
                && release.matches_running_ack(response)
    ) && old
        .worker
        .is_some_and(|worker| release.matches_running(worker))
        && new
            .worker
            .is_some_and(|worker| release.matches_running(worker))
        && !lifecycle
            .commands()
            .iter()
            .any(|entry| matches!(entry.request(), LocalCommandRequestV1::CancelRequested))
    {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}
