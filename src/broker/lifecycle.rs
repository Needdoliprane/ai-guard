//! Durable local facts. These data never grant policy or runtime authority.
use super::{
    checkpoint::{BrokerCheckpointV2, SupervisionOperationV1},
    journal::{JournalError, JournalResult, RevisionRefV1, canonical, digest, valid_hash},
};
use crate::{
    control::{
        ArmSubjectV1, DecisionPhaseV1, DecisionReceiptV1, HelperPermissionV1, HelperPurposeV1,
        WorkerTransitionV1,
    },
    model::Outcome,
    runner::CheckpointPayloadV1,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::{collections::HashSet, fmt};
use uuid::Uuid;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 64 * 1024;

fn require(valid: bool) -> JournalResult<()> {
    if valid {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}
fn equal<T: Serialize>(a: &T, b: &T) -> JournalResult<bool> {
    Ok(canonical(a)? == canonical(b)?)
}

macro_rules! closed {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Wire { $($variant),+ }
                Ok(match Wire::deserialize(decoder)
                    .map_err(|_| D::Error::custom("invalid private lifecycle code"))? {
                    $(Wire::$variant => Self::$variant),+
                })
            }
        }
    };
}
closed!(LocalFailurePhaseV1 {
    Decision,
    WorkspacePreparation,
    Admission,
    Import,
    ImportVerification,
    Worker,
    FinalScan,
    Export,
    Cleanup,
});
closed!(LocalFailureCodeV1 {
    InvalidData,
    Unavailable,
    IoFailed,
    ProtocolMismatch,
    TransportFailed,
});

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalCommandRequestV1 {
    ApprovalSubmitted {
        request_hash: String,
        review_receipt: DecisionReceiptV1,
    },
    CancelRequested,
    FailureObserved {
        phase: LocalFailurePhaseV1,
        code: LocalFailureCodeV1,
    },
}
impl<'de> Deserialize<'de> for LocalCommandRequestV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            ApprovalSubmitted {
                request_hash: String,
                review_receipt: DecisionReceiptV1,
            },
            CancelRequested {},
            FailureObserved {
                phase: LocalFailurePhaseV1,
                code: LocalFailureCodeV1,
            },
        }
        let wire = Wire::deserialize(decoder)
            .map_err(|_| D::Error::custom("invalid private local command"))?;
        let value = match wire {
            Wire::ApprovalSubmitted {
                request_hash,
                review_receipt,
            } => Self::ApprovalSubmitted {
                request_hash,
                review_receipt,
            },
            Wire::CancelRequested {} => Self::CancelRequested,
            Wire::FailureObserved { phase, code } => Self::FailureObserved { phase, code },
        };
        value
            .validate_shape()
            .map_err(|_| D::Error::custom("invalid private local command"))?;
        Ok(value)
    }
}
impl LocalCommandRequestV1 {
    fn validate_shape(&self) -> JournalResult<()> {
        if let Self::ApprovalSubmitted {
            request_hash,
            review_receipt,
        } = self
        {
            valid_hash(request_hash)?;
            valid_hash(&review_receipt.request_hash)?;
            valid_hash(&review_receipt.audit_hash)?;
            let id = Uuid::parse_str(&review_receipt.request_id)
                .map_err(|_| JournalError::InvalidInput)?;
            require(
                !id.is_nil()
                    && id.to_string() == review_receipt.request_id
                    && review_receipt.version == 1
                    && review_receipt.audit_seq > 0
                    && review_receipt.phase == DecisionPhaseV1::Initial
                    && *request_hash == review_receipt.request_hash,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCommandV1 {
    version: u32,
    command_id: Uuid,
    input_hash: String,
    request: LocalCommandRequestV1,
}
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointV3 {
    version: u32,
    broker: BrokerCheckpointV2,
    commands: Vec<LocalCommandV1>,
}
macro_rules! strict {
    ($name:ident { $($field:ident : $kind:ty),* $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Wire { $($field: $kind),* }
                let mut ignored = false;
                let wire: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
                    .map_err(|_| D::Error::custom("invalid private lifecycle checkpoint"))?;
                if ignored { return Err(D::Error::custom("invalid private lifecycle checkpoint")); }
                let value = Self { $($field: wire.$field),* };
                value.validate_shape().map_err(|_| D::Error::custom("invalid private lifecycle checkpoint"))?;
                Ok(value)
            }
        }
    };
}
strict!(LocalCommandV1 {
    version: u32,
    command_id: Uuid,
    input_hash: String,
    request: LocalCommandRequestV1
});
strict!(BrokerCheckpointV3 { version: u32, broker: BrokerCheckpointV2, commands: Vec<LocalCommandV1> });

impl LocalCommandV1 {
    pub fn new(
        operation_id: Uuid,
        intent_hash: &str,
        command_id: Uuid,
        request: LocalCommandRequestV1,
    ) -> JournalResult<Self> {
        let mut value = Self {
            version: 1,
            command_id,
            input_hash: String::new(),
            request,
        };
        value.input_hash = value.context_hash(operation_id, intent_hash)?;
        value.validate_shape()?;
        Ok(value)
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn input_hash(&self) -> &str {
        &self.input_hash
    }
    pub fn request(&self) -> &LocalCommandRequestV1 {
        &self.request
    }
    fn context_hash(&self, operation_id: Uuid, intent_hash: &str) -> JournalResult<String> {
        require(!operation_id.is_nil() && !self.command_id.is_nil())?;
        valid_hash(intent_hash)?;
        self.request.validate_shape()?;
        #[derive(Serialize)]
        struct Input<'a> {
            operation_id: Uuid,
            intent_hash: &'a str,
            command_id: Uuid,
            request: &'a LocalCommandRequestV1,
        }
        Ok(digest(
            "ai-guard:broker-local-command:v1",
            &canonical(&Input {
                operation_id,
                intent_hash,
                command_id: self.command_id,
                request: &self.request,
            })?,
        ))
    }
    fn validate_shape(&self) -> JournalResult<()> {
        if self.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        require(!self.command_id.is_nil())?;
        valid_hash(&self.input_hash)?;
        self.request.validate_shape()
    }
}

// Receipts acknowledge immutable durable facts; cloning never grants a fresh
// execution permission or consumes/reloads a start claim.
#[derive(Clone)]
pub struct LocalCommandReceiptV1 {
    pub(crate) command_id: Uuid,
    pub(crate) input_hash: String,
    pub(crate) head: RevisionRefV1,
}
impl LocalCommandReceiptV1 {
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn input_hash(&self) -> &str {
        &self.input_hash
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
opaque!(
    BrokerCheckpointV3,
    LocalCommandV1,
    LocalCommandRequestV1,
    LocalCommandReceiptV1
);

impl BrokerCheckpointV3 {
    pub fn new(broker: BrokerCheckpointV2, commands: Vec<LocalCommandV1>) -> JournalResult<Self> {
        let value = Self {
            version: 3,
            broker,
            commands,
        };
        value.validate_shape()?;
        Ok(value)
    }
    pub fn broker(&self) -> &BrokerCheckpointV2 {
        &self.broker
    }
    pub fn commands(&self) -> &[LocalCommandV1] {
        &self.commands
    }
    pub fn from_bytes(bytes: &[u8]) -> JournalResult<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(JournalError::TooLarge);
        }
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let value = Self::deserialize(&mut decoder).map_err(|_| JournalError::InvalidInput)?;
        decoder.end().map_err(|_| JournalError::InvalidInput)?;
        value.validate_shape()?;
        Ok(value)
    }
    pub fn canonical_bytes(&self) -> JournalResult<Vec<u8>> {
        self.validate_shape()?;
        canonical(self)
    }
    fn validate_shape(&self) -> JournalResult<()> {
        if self.version != 3 {
            return Err(JournalError::UnsupportedVersion);
        }
        if self.commands.len() > 64 {
            return Err(JournalError::LimitExceeded);
        }
        self.broker.canonical_bytes()?;
        let mut ids = HashSet::new();
        let mut approvals = 0;
        for command in &self.commands {
            command.validate_shape()?;
            require(ids.insert(command.command_id))?;
            approvals += usize::from(matches!(
                command.request,
                LocalCommandRequestV1::ApprovalSubmitted { .. }
            ));
        }
        require(approvals <= 1)?;
        if canonical(&self.commands)?.len() > MAX_COMMAND_BYTES
            || canonical(self)?.len() > MAX_BYTES
        {
            return Err(JournalError::TooLarge);
        }
        Ok(())
    }
    pub(crate) fn validate_context(
        &self,
        operation_id: Uuid,
        intent_hash: &str,
    ) -> JournalResult<()> {
        self.validate_shape()?;
        for command in &self.commands {
            require(command.input_hash == command.context_hash(operation_id, intent_hash)?)?;
        }
        Ok(())
    }
    pub(crate) fn validate_transition(
        &self,
        previous: Option<&Self>,
        previous_broker: Option<&BrokerCheckpointV2>,
        previous_runner: &CheckpointPayloadV1,
        last_cancel_operations: Option<usize>,
    ) -> JournalResult<()> {
        let old_commands = previous.map_or(&[][..], |old| old.commands.as_slice());
        require(self.commands.len() >= old_commands.len())?;
        for (old, new) in old_commands.iter().zip(&self.commands) {
            require(equal(old, new)?)?;
        }
        let old_operations = previous_broker.map_or(&[][..], |old| old.supervision().operations());
        let operations = self.broker.supervision().operations();
        let cancelled = self
            .commands
            .iter()
            .any(|c| matches!(c.request, LocalCommandRequestV1::CancelRequested));
        let current = self.broker.runner().broker_context();
        for command in &self.commands[old_commands.len()..] {
            if let LocalCommandRequestV1::ApprovalSubmitted {
                request_hash,
                review_receipt,
            } = &command.request
            {
                require(
                    !current.request.human_approved
                        && !cancelled
                        && !operations
                            .iter()
                            .any(|op| matches!(op, SupervisionOperationV1::Admission { .. })),
                )?;
                let mut found = false;
                for op in old_operations {
                    if let SupervisionOperationV1::Decision {
                        request,
                        response: Some(answer),
                    } = op
                        && !request.human_approved
                        && answer.decision().outcome == Outcome::Review
                        && request.request_id == review_receipt.request_id
                        && crate::policy::request_hash(request) == *request_hash
                        && equal(answer.receipt(), review_receipt)?
                    {
                        found = true;
                    }
                }
                require(found)?;
            }
        }
        if !previous_runner.broker_context().request.human_approved
            && current.request.human_approved
        {
            require(old_commands.iter().any(|command| {
                matches!(&command.request,
                LocalCommandRequestV1::ApprovalSubmitted { request_hash, .. }
                if current.request.approval_hash.as_ref() == Some(request_hash))
            }))?;
            require(operations.iter().skip(old_operations.len()).any(|op| {
                matches!(op,
                SupervisionOperationV1::Decision { request, response: None }
                if request.human_approved)
            }))?;
        }
        if cancelled {
            self.validate_cancel(
                previous_broker,
                previous_runner,
                old_operations,
                self.cancel_boundary(previous, last_cancel_operations)
                    .ok_or(JournalError::Conflict)?,
            )?;
        }
        Ok(())
    }
    pub(crate) fn cancel_boundary(
        &self,
        previous: Option<&Self>,
        last_boundary: Option<usize>,
    ) -> Option<usize> {
        let cancels = |value: &Self| {
            value
                .commands
                .iter()
                .filter(|command| matches!(command.request, LocalCommandRequestV1::CancelRequested))
                .count()
        };
        if cancels(self) > previous.map_or(0, cancels) {
            Some(self.broker.supervision().operations().len())
        } else {
            last_boundary
        }
    }
    fn validate_cancel(
        &self,
        previous_broker: Option<&BrokerCheckpointV2>,
        previous_runner: &CheckpointPayloadV1,
        old_operations: &[SupervisionOperationV1],
        boundary: usize,
    ) -> JournalResult<()> {
        let supervision = self.broker.supervision();
        let create_was_recorded = old_operations.iter().take(boundary).any(|op| {
            matches!(op,
            SupervisionOperationV1::WorkerMutation { request, .. }
            if matches!(request.transition, WorkerTransitionV1::CreateIssued))
        });
        for (index, operation) in supervision
            .operations()
            .iter()
            .enumerate()
            .skip(old_operations.len())
        {
            require(match operation {
                SupervisionOperationV1::Decision { .. }
                | SupervisionOperationV1::Admission { .. }
                | SupervisionOperationV1::WorkerClaim { .. } => false,
                SupervisionOperationV1::WorkerMutation { request, .. } => {
                    (!matches!(
                        request.transition,
                        WorkerTransitionV1::Prepare
                            | WorkerTransitionV1::CreateIssued
                            | WorkerTransitionV1::StartClaim { .. }
                    )) && (!matches!(request.transition, WorkerTransitionV1::Created { .. })
                        || create_was_recorded)
                }
                SupervisionOperationV1::Arm { request, .. } => match &request.subject {
                    ArmSubjectV1::Helper {
                        helper_id,
                        purpose: HelperPurposeV1::Export,
                        ..
                    } => self.export_grant_is_current(helper_id, boundary)?,
                    _ => false,
                },
                SupervisionOperationV1::HelperClaim { request, .. } => {
                    self.export_grant_is_current(&request.helper_id, boundary)?
                }
                SupervisionOperationV1::HelperObservation { .. }
                | SupervisionOperationV1::HelperCleanup { .. } => true,
                SupervisionOperationV1::Export { .. } => index >= boundary,
            })?;
        }
        let old_helpers = previous_broker.map_or(&[][..], |old| old.supervision().helpers());
        for helper in supervision.helpers().iter().skip(old_helpers.len()) {
            require(self.export_grant_is_current(&helper.policy().helper_id, boundary)?)?;
        }
        if previous_runner.broker_context().owned.is_none()
            && self.broker.runner().broker_context().owned.is_some()
        {
            require(create_was_recorded)?;
        }
        Ok(())
    }
    fn export_grant_is_current(&self, helper_id: &str, boundary: usize) -> JournalResult<bool> {
        let Some(helper) = self
            .broker
            .supervision()
            .helpers()
            .iter()
            .find(|helper| helper.policy().helper_id == helper_id)
        else {
            return Ok(false);
        };
        if helper.policy().purpose != HelperPurposeV1::Export {
            return Ok(false);
        }
        let HelperPermissionV1::Export { receipt, .. } = helper.permission() else {
            return Ok(false);
        };
        for operation in self.broker.supervision().operations().iter().skip(boundary) {
            if let SupervisionOperationV1::Export {
                response: Some(given),
                ..
            } = operation
                && equal(receipt, given)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
