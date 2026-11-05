use super::*;
use crate::control::kubernetes_admission::{
    KubernetesCreateOutcomeV1, KubernetesCreateStateV1, KubernetesCreateTargetV1,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const FILES: [(&str, u64); 7] = [
    (".writer.lock", 0),
    ("pod.json", 32 * 1024),
    ("prepared.json", 32 * 1024),
    ("emission.json", 16 * 1024),
    ("candidate.json", 64 * 1024),
    ("refusal.json", 1024),
    ("ack.json", 16 * 1024),
];
pub(super) const TOTAL_BYTES: u64 = 161 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreparedRecord {
    pub version: u32,
    pub tenant_id: String,
    pub intent: KubernetesCreateIntentV1,
    pub target: KubernetesCreateTargetV1,
    pub reservation: KubernetesCreateReceiptV1,
    pub claim_request: KubernetesCreateClaimV1,
    pub pod_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmissionRecord {
    pub version: u32,
    pub prepared_sha256: String,
    pub claim: KubernetesCreateClaimV1,
    pub receipt: KubernetesCreateReceiptV1,
    pub snapshot: KubernetesCreateSnapshotV1,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RefusalRecord {
    pub version: u32,
    pub candidate_sha256: String,
    pub refusal: CandidateRefusal,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AckRecord {
    pub version: u32,
    pub prepared_sha256: String,
    pub candidate_sha256: String,
    pub receipt: KubernetesCreateReceiptV1,
}

pub(super) fn require(condition: bool) -> JournalResult<()> {
    if condition {
        Ok(())
    } else {
        Err(JournalError::InvalidInput)
    }
}
pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(super) fn encode<T: Serialize>(record: &T, maximum: u64) -> JournalResult<Vec<u8>> {
    let value = serde_json::to_value(record).map_err(|_| JournalError::InvalidInput)?;
    super::super::json::canonical_bytes(&value, maximum as usize)
        .map_err(|_| JournalError::Capacity)
}
pub(super) fn decode<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    maximum: u64,
) -> JournalResult<T> {
    if bytes.len() as u64 > maximum {
        return Err(JournalError::Corrupt);
    }
    let value = super::super::json::decode(bytes).map_err(|_| JournalError::Corrupt)?;
    let decoded = serde_json::from_value(value).map_err(|_| JournalError::Corrupt)?;
    if encode(&decoded, maximum).map_err(|_| JournalError::Corrupt)? != bytes {
        return Err(JournalError::Corrupt);
    }
    Ok(decoded)
}

impl PreparedRecord {
    pub fn validate(&self, pod: &[u8]) -> JournalResult<()> {
        crate::tenancy::validate_tenant_id(&self.tenant_id)
            .map_err(|_| JournalError::InvalidInput)?;
        self.intent
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        self.reservation
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        self.claim_request
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        let rendered =
            super::super::prepare(&self.intent).map_err(|_| JournalError::InvalidInput)?;
        let intent_hash = self
            .intent
            .intent_hash()
            .map_err(|_| JournalError::InvalidInput)?;
        require(
            self.version == 1
                && self.target == *rendered.target()
                && pod == rendered.pod_bytes()
                && self.pod_sha256 == digest(pod)
                && self.reservation.operation_id == self.intent.create_id
                && self.reservation.input_hash == intent_hash
                && self.reservation.control_namespace_uuid == self.intent.control_namespace_uuid
                && self.reservation.emission_window.is_none()
                && self.claim_request.create_id == self.intent.create_id
                && self.claim_request.intent_hash == intent_hash
                && self.claim_request.expected_revision == 1
                && self.claim_request.expected_create_epoch == 1,
        )
    }
    pub fn reserved_snapshot(&self) -> KubernetesCreateSnapshotV1 {
        KubernetesCreateSnapshotV1 {
            version: 1,
            intent: self.intent.clone(),
            intent_hash: self.claim_request.intent_hash.clone(),
            target: self.target.clone(),
            state: KubernetesCreateStateV1::Reserved,
            create_epoch: 1,
            revision: 1,
            quota_held: true,
            claim: None,
            emission_window: None,
            last_result: None,
            last_cancel: None,
        }
    }
}
impl EmissionRecord {
    pub fn validate(&self, prepared: &PreparedRecord, prepared_sha256: &str) -> JournalResult<()> {
        self.receipt
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        let mut expected = prepared.reserved_snapshot();
        expected.state = KubernetesCreateStateV1::EmissionPossible;
        expected.revision = 2;
        expected.claim = Some(prepared.claim_request.clone());
        expected.emission_window = self.receipt.emission_window.clone();
        require(
            self.version == 1
                && self.prepared_sha256 == prepared_sha256
                && self.claim == prepared.claim_request
                && self.receipt.operation_id == self.claim.operation_id
                && self.receipt.input_hash
                    == self
                        .claim
                        .input_hash()
                        .map_err(|_| JournalError::InvalidInput)?
                && self.receipt.control_namespace_uuid == prepared.intent.control_namespace_uuid
                && self.receipt.audit_seq > prepared.reservation.audit_seq
                && self.receipt.emission_window.is_some()
                && self.snapshot == expected,
        )
    }
}
pub(super) fn validate_candidate(
    prepared: &PreparedRecord,
    emission: &EmissionRecord,
    candidate: &KubernetesCreateResultV1,
) -> JournalResult<()> {
    candidate
        .validate()
        .map_err(|_| JournalError::InvalidInput)?;
    require(
        candidate.create_id == prepared.intent.create_id
            && candidate.intent_hash == prepared.claim_request.intent_hash
            && candidate.claim_operation_id == emission.claim.operation_id
            && candidate.expected_revision == emission.snapshot.revision
            && candidate.expected_create_epoch == emission.snapshot.create_epoch,
    )?;
    if let KubernetesCreateOutcomeV1::CreatedHeld {
        observation,
        admitted_spec,
        admitted_configuration_sha256,
        ..
    } = &candidate.outcome
    {
        super::super::validate_created_held(
            &prepared.intent,
            observation,
            admitted_spec,
            admitted_configuration_sha256,
        )
        .map_err(|_| JournalError::InvalidInput)?;
    }
    Ok(())
}
pub(super) fn validate_ack(
    prepared: &PreparedRecord,
    prepared_sha256: &str,
    emission: &EmissionRecord,
    candidate: &KubernetesCreateResultV1,
    candidate_sha256: &str,
    ack: &AckRecord,
) -> JournalResult<()> {
    ack.receipt
        .validate()
        .map_err(|_| JournalError::InvalidInput)?;
    require(
        ack.version == 1
            && ack.prepared_sha256 == prepared_sha256
            && ack.candidate_sha256 == candidate_sha256
            && ack.receipt.operation_id == candidate.operation_id
            && ack.receipt.input_hash
                == candidate
                    .input_hash()
                    .map_err(|_| JournalError::InvalidInput)?
            && ack.receipt.control_namespace_uuid == prepared.intent.control_namespace_uuid
            && ack.receipt.emission_window.is_none()
            && ack.receipt.audit_seq > emission.receipt.audit_seq,
    )
}
pub(super) fn result_snapshot(
    emission: &EmissionRecord,
    candidate: &KubernetesCreateResultV1,
) -> JournalResult<KubernetesCreateSnapshotV1> {
    let mut snapshot = emission.snapshot.clone();
    snapshot.revision = snapshot
        .revision
        .checked_add(1)
        .ok_or(JournalError::InvalidInput)?;
    snapshot.state = match candidate.outcome {
        KubernetesCreateOutcomeV1::CreatedHeld { .. } => KubernetesCreateStateV1::CreatedHeld,
        KubernetesCreateOutcomeV1::OwnershipUnknown { .. } => {
            KubernetesCreateStateV1::OwnershipUnknown
        }
    };
    snapshot.last_result = Some(candidate.clone());
    Ok(snapshot)
}

pub(super) fn screen_value(value: &Value, secrets: &[String]) -> JournalResult<()> {
    match value {
        Value::String(value) => screen(value.as_bytes(), secrets),
        Value::Object(fields) => {
            for (key, value) in fields {
                screen(key.as_bytes(), secrets)?;
                screen_value(value, secrets)?;
            }
            Ok(())
        }
        Value::Array(values) => {
            for value in values {
                screen_value(value, secrets)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
pub(super) fn screen(bytes: &[u8], secrets: &[String]) -> JournalResult<()> {
    if secrets.iter().any(|secret| {
        bytes
            .windows(secret.len())
            .any(|part| part == secret.as_bytes())
    }) {
        Err(JournalError::UnsafeMaterial)
    } else {
        Ok(())
    }
}
