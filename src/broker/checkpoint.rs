//! Private checkpoint facts. Deserialization never grants execution authority.
use super::{
    intent::ExecutionIntentV1,
    journal::{BindingManifestV1, JournalError, JournalResult, canonical},
};
use crate::{
    control::*,
    model::{Decision, DecisionRequest, Outcome},
    runner::CheckpointPayloadV1,
    runtime::OwnedContainerSnapshot,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::{collections::HashSet, fmt};
use uuid::Uuid;
mod validation;

const MAX_BYTES: usize = 32 * 1024 * 1024;
fn ensure(valid: bool) -> JournalResult<()> {
    if valid {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}
fn checked<T>(value: anyhow::Result<T>) -> JournalResult<T> {
    value.map_err(|_| JournalError::Conflict)
}
fn equal<T: Serialize>(a: &T, b: &T) -> JournalResult<bool> {
    Ok(canonical(a)? == canonical(b)?)
}
fn uuid(value: &str) -> JournalResult<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| JournalError::InvalidInput)?;
    ensure(!id.is_nil() && id.to_string() == value)?;
    Ok(id)
}
fn hash(value: &str) -> JournalResult<()> {
    ensure(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )
}
fn receipt(value: &MutationReceiptV1) -> JournalResult<()> {
    uuid(&value.operation_id)?;
    hash(&value.input_hash)?;
    hash(&value.audit_hash)?;
    ensure(value.audit_seq > 0 && value.revision.is_none_or(|r| r > 0))?;
    if let Some(worker) = &value.worker_id {
        uuid(worker)?;
    }
    Ok(())
}
fn decision_receipt(value: &DecisionReceiptV1) -> JournalResult<()> {
    ensure(value.version == 1 && value.audit_seq > 0)?;
    uuid(&value.request_id)?;
    hash(&value.request_hash)?;
    hash(&value.audit_hash)
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointV2 {
    version: u32,
    runner: CheckpointPayloadV1,
    supervision: SupervisionCheckpointV1,
}
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisionCheckpointV1 {
    version: u32,
    control_namespace_uuid: Uuid,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<SupervisionOperationV1>,
}
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperPreparationV1 {
    policy: HelperExecutionPolicyV1,
    permission: HelperPermissionV1,
    owned: Option<OwnedContainerSnapshot>,
}
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAnswerV1 {
    decision: Decision,
    receipt: DecisionReceiptV1,
}
fn deserialize_owned<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<OwnedContainerSnapshot, D::Error> {
    // The internally tagged operation has already buffered its content. Install
    // the ignored-field guard here, around the historical untagged structs.
    let mut ignored = false;
    let value = serde_ignored::deserialize(decoder, |_| ignored = true)
        .map_err(|_| D::Error::custom("invalid private owned snapshot"))?;
    if ignored {
        return Err(D::Error::custom("invalid private owned snapshot"));
    }
    Ok(value)
}
macro_rules! operation_types {
    ($($variant:ident { $($(#[$attribute:meta])* $field:ident: $kind:ty),* $(,)? }),* $(,)?) => {
        #[derive(Clone, Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        #[allow(clippy::large_enum_variant, reason = "Owned DTO fields are fixed by the checkpoint contract; operation count and persisted bytes are bounded")]
        pub enum SupervisionOperationV1 { $($variant { $($field: $kind),* }),* }
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        #[allow(clippy::large_enum_variant, reason = "This private wire preserves the same bounded owned DTO contract as SupervisionOperationV1")]
        enum OperationWire { $($variant { $($(#[$attribute])* $field: $kind),* }),* }
        impl<'de> Deserialize<'de> for SupervisionOperationV1 {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let mut ignored = false;
                let wire: OperationWire = serde_ignored::deserialize(decoder, |_| ignored = true)
                    .map_err(|_| D::Error::custom("invalid private supervision operation"))?;
                if ignored { return Err(D::Error::custom("invalid private supervision operation")); }
                let value = match wire { $(OperationWire::$variant { $($field),* } => Self::$variant { $($field),* }),* };
                value.validate_shape().map_err(|_| D::Error::custom("invalid private supervision operation"))?;
                Ok(value)
            }
        }
    };
}
operation_types! {
    Decision { request: DecisionRequest, response: Option<DecisionAnswerV1> },
    Admission { request: SupervisedAdmissionV1, response: Option<AdmissionResultV1> },
    WorkerMutation { request: WorkerMutationV1, response: Option<MutationResultV1> },
    Arm { parent: WorkerV1, #[serde(deserialize_with = "deserialize_owned")] owned: OwnedContainerSnapshot, request: ArmV1, response: Option<ArmResultV1> },
    WorkerClaim { request: ArmedStartClaimV1, response: Option<MutationResultV1> },
    HelperClaim { request: HelperStartClaimV1, response: Option<HelperClaimResultV1> },
    HelperObservation { request: HelperObservationV1, response: Option<MutationReceiptV1> },
    HelperCleanup { request: HelperCleanupV1, response: Option<MutationReceiptV1> },
    Export { request: ExportRequestV1, response: Option<MutationReceiptV1> },
}
macro_rules! strict_record {
    ($name:ident { $($field:ident: $kind:ty),* $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Wire { $($field: $kind),* }
                let mut ignored = false;
                let wire: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
                    .map_err(|_| D::Error::custom("invalid private broker checkpoint"))?;
                if ignored { return Err(D::Error::custom("invalid private broker checkpoint")); }
                let value = Self { $($field: wire.$field),* };
                value.validate_shape().map_err(|_| D::Error::custom("invalid private broker checkpoint"))?;
                Ok(value)
            }
        }
    };
}
strict_record!(BrokerCheckpointV2 {
    version: u32,
    runner: CheckpointPayloadV1,
    supervision: SupervisionCheckpointV1
});
strict_record!(SupervisionCheckpointV1 { version: u32, control_namespace_uuid: Uuid, helpers: Vec<HelperPreparationV1>, operations: Vec<SupervisionOperationV1> });
strict_record!(HelperPreparationV1 { policy: HelperExecutionPolicyV1, permission: HelperPermissionV1, owned: Option<OwnedContainerSnapshot> });
strict_record!(DecisionAnswerV1 {
    decision: Decision,
    receipt: DecisionReceiptV1
});
macro_rules! opaque {
    ($($kind:ty),+ $(,)?) => {$(impl fmt::Debug for $kind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(concat!(stringify!($kind), "(<private>)"))
        }
    })+};
}
opaque!(
    BrokerCheckpointV2,
    SupervisionCheckpointV1,
    HelperPreparationV1,
    DecisionAnswerV1,
    SupervisionOperationV1
);

impl BrokerCheckpointV2 {
    pub fn new(
        runner: CheckpointPayloadV1,
        supervision: SupervisionCheckpointV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 2,
            runner,
            supervision,
        };
        value.canonical_bytes()?;
        Ok(value)
    }
    pub fn runner(&self) -> &CheckpointPayloadV1 {
        &self.runner
    }
    pub fn supervision(&self) -> &SupervisionCheckpointV1 {
        &self.supervision
    }
    pub fn from_bytes(bytes: &[u8]) -> JournalResult<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(JournalError::TooLarge);
        }
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let mut ignored = false;
        let value: Self = serde_ignored::deserialize(&mut decoder, |_| ignored = true)
            .map_err(|_| JournalError::InvalidInput)?;
        decoder.end().map_err(|_| JournalError::InvalidInput)?;
        if ignored {
            return Err(JournalError::InvalidInput);
        }
        value.canonical_bytes()?;
        Ok(value)
    }
    pub fn canonical_bytes(&self) -> JournalResult<Vec<u8>> {
        let bytes = self.validated_canonical_bytes()?;
        if bytes.len() > MAX_BYTES {
            return Err(JournalError::TooLarge);
        }
        Ok(bytes)
    }
    fn validate_shape(&self) -> JournalResult<()> {
        self.validated_canonical_bytes().map(drop)
    }
    fn validated_canonical_bytes(&self) -> JournalResult<Vec<u8>> {
        if self.version != 2 {
            return Err(JournalError::UnsupportedVersion);
        }
        self.runner.canonical_bytes()?;
        self.supervision.validate_shape()?;
        let bytes = canonical(self)?;
        if bytes.len() > MAX_BYTES {
            return Err(JournalError::TooLarge);
        }
        Ok(bytes)
    }
    pub(crate) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        self.validate_shape()?;
        let (old_helpers, old_operations) = if let Some(old) = previous {
            ensure(
                self.supervision.control_namespace_uuid == old.supervision.control_namespace_uuid,
            )?;
            ensure(
                !old.runner.broker_context().request.human_approved
                    || self.runner.broker_context().request.human_approved,
            )?;
            (
                old.supervision.helpers.as_slice(),
                old.supervision.operations.as_slice(),
            )
        } else {
            (&[][..], &[][..])
        };
        ensure(
            self.supervision.helpers.len() >= old_helpers.len()
                && self.supervision.operations.len() >= old_operations.len(),
        )?;
        for (index, helper) in self.supervision.helpers.iter().enumerate() {
            if let Some(old) = old_helpers.get(index) {
                ensure(
                    equal(&old.policy, &helper.policy)?
                        && equal(&old.permission, &helper.permission)?,
                )?;
                if old.owned.is_some() {
                    ensure(equal(&old.owned, &helper.owned)?)?;
                }
            } else {
                ensure(helper.owned.is_none())?;
            }
        }
        for (index, operation) in self.supervision.operations.iter().enumerate() {
            if let Some(old) = old_operations.get(index) {
                ensure(equal(
                    &old.without_response(),
                    &operation.without_response(),
                )?)?;
                if old.has_response() {
                    ensure(equal(old, operation)?)?;
                }
            } else {
                ensure(!operation.has_response())?;
            }
        }
        Ok(())
    }
}
impl SupervisionCheckpointV1 {
    pub fn new(
        control_namespace_uuid: Uuid,
        helpers: Vec<HelperPreparationV1>,
        operations: Vec<SupervisionOperationV1>,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            control_namespace_uuid,
            helpers,
            operations,
        };
        value.validate_shape()?;
        Ok(value)
    }
    pub fn control_namespace_uuid(&self) -> Uuid {
        self.control_namespace_uuid
    }
    pub fn helpers(&self) -> &[HelperPreparationV1] {
        &self.helpers
    }
    pub fn operations(&self) -> &[SupervisionOperationV1] {
        &self.operations
    }
    fn validate_shape(&self) -> JournalResult<()> {
        if self.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        ensure(!self.control_namespace_uuid.is_nil())?;
        if self.helpers.len() > 32
            || self.operations.len() > 128
            || self
                .operations
                .iter()
                .filter(|op| !op.has_response())
                .count()
                > 64
        {
            return Err(JournalError::LimitExceeded);
        }
        let mut helpers = HashSet::new();
        for helper in &self.helpers {
            checked(helper.policy.validate())?;
            ensure(helpers.insert(helper.policy.helper_id.as_str()))?;
        }
        let mut keys = HashSet::new();
        let mut arm_subjects = HashSet::new();
        let mut claims = HashSet::new();
        let mut destinations = HashSet::new();
        let mut decisions = 0;
        let mut admissions = 0;
        for operation in &self.operations {
            operation.validate_shape()?;
            ensure(keys.insert(operation.key()))?;
            match operation {
                SupervisionOperationV1::Arm { request, .. } => {
                    ensure(arm_subjects.insert(checked(request.subject_key())?))?
                }
                SupervisionOperationV1::WorkerClaim { request, .. } => {
                    ensure(claims.insert(format!("worker:{}", request.attempt_id)))?
                }
                SupervisionOperationV1::HelperClaim { request, .. } => {
                    ensure(claims.insert(format!("helper:{}", request.helper_id)))?
                }
                SupervisionOperationV1::Export { request, .. } => {
                    ensure(destinations.insert(request.destination_ref.as_str()))?
                }
                _ => {}
            }
            decisions += usize::from(matches!(operation, SupervisionOperationV1::Decision { .. }));
            admissions += usize::from(matches!(
                operation,
                SupervisionOperationV1::Admission { .. }
            ));
        }
        ensure(decisions <= 2 && admissions <= 1)
    }
}
impl HelperPreparationV1 {
    pub fn new(
        policy: HelperExecutionPolicyV1,
        permission: HelperPermissionV1,
        owned: Option<OwnedContainerSnapshot>,
    ) -> JournalResult<Self> {
        let value = Self {
            policy,
            permission,
            owned,
        };
        value.validate_shape()?;
        Ok(value)
    }
    pub fn policy(&self) -> &HelperExecutionPolicyV1 {
        &self.policy
    }
    pub fn permission(&self) -> &HelperPermissionV1 {
        &self.permission
    }
    pub fn owned(&self) -> Option<&OwnedContainerSnapshot> {
        self.owned.as_ref()
    }
    fn validate_shape(&self) -> JournalResult<()> {
        checked(self.policy.validate())?;
        match &self.permission {
            HelperPermissionV1::Admission { receipt: given } => receipt(given)?,
            HelperPermissionV1::ImportVerification {
                import_helper_id,
                import_arm,
                import_terminal,
                manifest_digest,
            } => {
                uuid(import_helper_id)?;
                checked(import_arm.validate())?;
                receipt(import_terminal)?;
                hash(manifest_digest)?;
            }
            HelperPermissionV1::Export {
                receipt: given,
                destination_ref,
                destination_intent_hash,
            } => {
                receipt(given)?;
                uuid(destination_ref)?;
                hash(destination_intent_hash)?;
            }
        }
        if let Some(owned) = &self.owned {
            hash(owned.handle.id())?;
            hash(owned.handle.authority_fingerprint())?;
            ensure(
                matches!(owned.handle.fingerprint_version(), 1..=3)
                    && matches!(
                        owned
                            .observation
                            .configuration_fingerprint_version
                            .unwrap_or(1),
                        1..=3
                    ),
            )?;
            checked(
                owned
                    .observation
                    .validate_identity_allocation(&owned.observation),
            )?;
        }
        ensure(canonical(self)?.len() <= 512 * 1024)
    }
}
impl DecisionAnswerV1 {
    pub fn new(decision: Decision, receipt: DecisionReceiptV1) -> JournalResult<Self> {
        let value = Self { decision, receipt };
        value.validate_shape()?;
        Ok(value)
    }
    fn validate_shape(&self) -> JournalResult<()> {
        decision_receipt(&self.receipt)?;
        for text in [
            &self.decision.engine,
            &self.decision.rule,
            &self.decision.reason,
        ] {
            ensure(!text.is_empty() && text.len() <= 65536 && !text.contains('\0'))?;
        }
        Ok(())
    }
    pub fn decision(&self) -> &Decision {
        &self.decision
    }
    pub fn receipt(&self) -> &DecisionReceiptV1 {
        &self.receipt
    }
}
impl SupervisionOperationV1 {
    pub(crate) fn has_response(&self) -> bool {
        match self {
            Self::Decision { response, .. } => response.is_some(),
            Self::Admission { response, .. } => response.is_some(),
            Self::WorkerMutation { response, .. } => response.is_some(),
            Self::Arm { response, .. } => response.is_some(),
            Self::WorkerClaim { response, .. } => response.is_some(),
            Self::HelperClaim { response, .. } => response.is_some(),
            Self::HelperObservation { response, .. }
            | Self::HelperCleanup { response, .. }
            | Self::Export { response, .. } => response.is_some(),
        }
    }
    fn without_response(&self) -> Self {
        let mut value = self.clone();
        match &mut value {
            Self::Decision { response, .. } => *response = None,
            Self::Admission { response, .. } => *response = None,
            Self::WorkerMutation { response, .. } => *response = None,
            Self::Arm { response, .. } => *response = None,
            Self::WorkerClaim { response, .. } => *response = None,
            Self::HelperClaim { response, .. } => *response = None,
            Self::HelperObservation { response, .. }
            | Self::HelperCleanup { response, .. }
            | Self::Export { response, .. } => *response = None,
        }
        value
    }
    fn key(&self) -> String {
        match self {
            Self::Decision { request, .. } => {
                format!("decision:{}:{}", request.request_id, request.human_approved)
            }
            Self::Admission { request, .. } => {
                format!("mutation:{}", request.admission.operation_id)
            }
            Self::WorkerMutation { request, .. } => format!("mutation:{}", request.operation_id),
            Self::Arm { request, .. } => format!("mutation:{}", request.operation_id),
            Self::WorkerClaim { request, .. } => format!("mutation:{}", request.operation_id),
            Self::HelperClaim { request, .. } => format!("mutation:{}", request.operation_id),
            Self::HelperObservation { request, .. } => format!("mutation:{}", request.operation_id),
            Self::HelperCleanup { request, .. } => format!("mutation:{}", request.operation_id),
            Self::Export { request, .. } => format!("mutation:{}", request.operation_id),
        }
    }
    fn validate_shape(&self) -> JournalResult<()> {
        match self {
            Self::Decision { request, response } => {
                uuid(&request.request_id)?;
                if let Some(response) = response {
                    DecisionAnswerV1::new(response.decision.clone(), response.receipt.clone())?;
                }
            }
            Self::Admission { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(&response.receipt)?;
                }
            }
            Self::WorkerMutation { request, response } => {
                ensure(
                    request.version == 1
                        && request.expected_revision > 0
                        && !matches!(request.transition, WorkerTransitionV1::StartClaim { .. }),
                )?;
                uuid(&request.operation_id)?;
                uuid(&request.worker_id)?;
                uuid(&request.attempt_id)?;
                if let Some(response) = response {
                    receipt(&response.receipt)?;
                }
            }
            Self::Arm {
                request, response, ..
            } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    checked(response.receipt.validate_for_arm(request))?;
                    ensure(equal(request, &response.arm)?)?;
                }
            }
            Self::WorkerClaim { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(&response.receipt)?;
                }
            }
            Self::HelperClaim { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(&response.receipt)?;
                }
            }
            Self::HelperObservation { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(response)?;
                }
            }
            Self::HelperCleanup { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(response)?;
                }
            }
            Self::Export { request, response } => {
                checked(request.validate())?;
                if let Some(response) = response {
                    receipt(response)?;
                }
            }
        }
        ensure(canonical(self)?.len() <= 512 * 1024)
    }
}
