//! Private held-CREATE retention, separate from legacy execution checkpoints.
//! No disk data or method grants a CREATE permission. Partial writes are retained
//! and refused on reopen; there is deliberately no repair or overwrite path.
use super::PreparedHeldCreate;
use crate::audit::{ConfirmedKubernetesCreateResult, FreshKubernetesCreateClaimCommit};
use crate::broker::held_create_client::{ConfirmedHeldCreateResultAck, LiveHeldCreateClaimAck};
use crate::control::kubernetes_admission::{
    KubernetesCreateClaimV1, KubernetesCreateIntentV1, KubernetesCreateReceiptV1,
    KubernetesCreateResultV1, KubernetesCreateSnapshotV1,
};
use crate::control_fs::{Directory, PrivateFile};
use crate::runtime::kubernetes::KubernetesPodHandle;
use records::{AckRecord, EmissionRecord, PreparedRecord, RefusalRecord};
use serde::{Deserialize, Serialize};

#[path = "admission_journal_fs.rs"]
mod fs;
#[path = "admission_journal_records.rs"]
mod records;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JournalError {
    InvalidInput,
    Conflict,
    Corrupt,
    Capacity,
    Busy,
    Unavailable,
    UnsafeMaterial,
}
pub(crate) type JournalResult<T> = Result<T, JournalError>;
impl std::fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "invalid held CREATE journal input",
            Self::Conflict => "held CREATE journal conflicts with retained content",
            Self::Corrupt => "held CREATE journal requires reconciliation",
            Self::Capacity => "held CREATE journal exceeds its fixed limits",
            Self::Busy => "held CREATE journal is already owned",
            Self::Unavailable => "held CREATE journal durability is unavailable",
            Self::UnsafeMaterial => "held CREATE journal protection refused material",
        })
    }
}
impl std::error::Error for JournalError {}

pub(crate) struct AdmissionJournalProtection {
    files: Vec<PrivateFile>,
    secrets: Vec<String>,
    excluded: Vec<fs::ExcludedFile>,
}
impl AdmissionJournalProtection {
    pub(crate) fn new(files: Vec<PrivateFile>, secrets: Vec<String>) -> Self {
        Self {
            files,
            secrets: secrets
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            excluded: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateRefusal {
    StaleRevision,
    StaleEpoch,
    ControlRejected,
    Unavailable,
}

pub(crate) struct AdmissionJournal {
    disk: fs::Disk,
    prepared: PreparedRecord,
    emission: Option<EmissionRecord>,
    candidate: Option<KubernetesCreateResultV1>,
    refusal: Option<RefusalRecord>,
    ack: Option<AckRecord>,
    recovered: bool,
}
/// A local durability fact, never a permission and never deserializable.
pub(crate) struct EmissionCheckpoint {
    tenant_id: String,
    create_id: String,
    intent_hash: String,
    claim_operation_id: String,
    create_epoch: u64,
    request_sha256: String,
    emission_sha256: String,
}
pub(crate) struct RetainedCandidate {
    operation_id: String,
    input_hash: String,
    refusal: Option<CandidateRefusal>,
}
impl RetainedCandidate {
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub(crate) fn input_hash(&self) -> &str {
        &self.input_hash
    }
    pub(crate) fn refusal(&self) -> Option<CandidateRefusal> {
        self.refusal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryPhase {
    Prepared,
    ObserveOnly,
    CandidateRetained,
    ResultAcknowledged,
}
pub(crate) struct RecoveredHeldCreate {
    journal: AdmissionJournal,
    phase: RecoveryPhase,
}
impl RecoveredHeldCreate {
    pub(crate) fn phase(&self) -> RecoveryPhase {
        self.phase
    }
    pub(crate) fn into_journal(self) -> AdmissionJournal {
        self.journal
    }
}

impl AdmissionJournal {
    pub(crate) fn prepare(
        root: Directory,
        protection: AdmissionJournalProtection,
        tenant_id: &str,
        intent: &KubernetesCreateIntentV1,
        prepared: PreparedHeldCreate,
        reservation: &KubernetesCreateReceiptV1,
        reserved: &KubernetesCreateSnapshotV1,
        claim_request: &KubernetesCreateClaimV1,
    ) -> JournalResult<Self> {
        protection.exclusions(&root)?;
        let record = PreparedRecord {
            version: 1,
            tenant_id: tenant_id.into(),
            intent: intent.clone(),
            target: prepared.target().clone(),
            reservation: reservation.clone(),
            claim_request: claim_request.clone(),
            pod_sha256: records::digest(prepared.pod_bytes()),
        };
        record.validate(prepared.pod_bytes())?;
        records::require(*reserved == record.reserved_snapshot())?;
        let bytes = records::encode(&record, 32 * 1024)?;
        // All material, including decoded strings and target bytes, is checked
        // before even the empty lock is created.
        protection.screen_record(&bytes)?;
        protection.screen_record(prepared.pod_bytes())?;
        let mut disk =
            fs::Disk::acquire(root, protection, true)?.ok_or(JournalError::Unavailable)?;
        disk.append("pod.json", prepared.pod_bytes())?;
        disk.append("prepared.json", &bytes)?;
        disk.revalidate()?;
        Ok(Self {
            disk,
            prepared: record,
            emission: None,
            candidate: None,
            refusal: None,
            ack: None,
            recovered: false,
        })
    }

    /// Only the real client can construct this distinct live HTTP proof.
    pub(crate) fn commit_remote_emission_possible(
        &mut self,
        live: &LiveHeldCreateClaimAck,
    ) -> JournalResult<EmissionCheckpoint> {
        // Caller owns bounded FS admission. HTTP provenance stays a separate
        // opaque type; no wire value or disk record manufactures StoreFresh.
        live.revalidate_material()
            .map_err(|_| JournalError::UnsafeMaterial)?;
        self.commit_emission_record(
            live.tenant_id(),
            live.claim(),
            live.receipt(),
            live.snapshot(),
        )
    }

    pub(crate) fn commit_emission_possible(
        &mut self,
        fresh: &FreshKubernetesCreateClaimCommit,
    ) -> JournalResult<EmissionCheckpoint> {
        self.commit_emission_record(
            fresh.tenant_id(),
            fresh.claim(),
            fresh.receipt(),
            fresh.snapshot(),
        )
    }

    fn commit_emission_record(
        &mut self,
        tenant_id: &str,
        claim: &KubernetesCreateClaimV1,
        receipt: &KubernetesCreateReceiptV1,
        snapshot: &KubernetesCreateSnapshotV1,
    ) -> JournalResult<EmissionCheckpoint> {
        self.disk.revalidate()?;
        if self.recovered || self.emission.is_some() {
            return Err(JournalError::Conflict);
        }
        records::require(tenant_id == self.prepared.tenant_id)?;
        // Only the two opaque proof entry points above reach this private
        // persistence routine. Disk records can never call the transition.
        let record = EmissionRecord {
            version: 1,
            prepared_sha256: self.disk.commitment("prepared.json")?.into(),
            claim: claim.clone(),
            receipt: receipt.clone(),
            snapshot: snapshot.clone(),
        };
        record.validate(&self.prepared, self.disk.commitment("prepared.json")?)?;
        if self.candidate.is_some() || self.ack.is_some() {
            return Err(JournalError::Conflict);
        }
        let bytes = records::encode(&record, 16 * 1024)?;
        self.disk.append("emission.json", &bytes)?;
        self.emission = Some(record);
        Ok(EmissionCheckpoint {
            tenant_id: self.prepared.tenant_id.clone(),
            create_id: self.prepared.intent.create_id.clone(),
            intent_hash: self.prepared.claim_request.intent_hash.clone(),
            claim_operation_id: self.prepared.claim_request.operation_id.clone(),
            create_epoch: self.prepared.claim_request.expected_create_epoch,
            request_sha256: self.prepared.target.request_sha256.clone(),
            emission_sha256: self.disk.commitment("emission.json")?.into(),
        })
    }

    pub(crate) fn retain_candidate(
        &mut self,
        candidate: &KubernetesCreateResultV1,
        refusal: Option<CandidateRefusal>,
    ) -> JournalResult<RetainedCandidate> {
        self.disk.revalidate()?;
        if self
            .candidate
            .as_ref()
            .is_some_and(|retained| retained != candidate)
        {
            return Err(JournalError::Conflict);
        }
        if let Some(code) = refusal {
            if self
                .refusal
                .as_ref()
                .is_some_and(|retained| retained.refusal != code)
            {
                return Err(JournalError::Conflict);
            }
        }
        let emission = self.emission.as_ref().ok_or(JournalError::InvalidInput)?;
        records::validate_candidate(&self.prepared, emission, candidate)?;
        let bytes = records::encode(candidate, 64 * 1024)?;
        // Preflight both envelopes before writing the first record.
        let refusal_record = refusal.map(|refusal| RefusalRecord {
            version: 1,
            candidate_sha256: records::digest(&bytes),
            refusal,
        });
        let refusal_bytes = refusal_record
            .as_ref()
            .map(|record| records::encode(record, 1024))
            .transpose()?;
        self.disk.protection.screen_record(&bytes)?;
        if let Some(bytes) = &refusal_bytes {
            self.disk.protection.screen_record(bytes)?;
        }
        self.disk.append("candidate.json", &bytes)?;
        self.candidate = Some(candidate.clone());
        if let Some(bytes) = refusal_bytes {
            self.disk.append("refusal.json", &bytes)?;
            self.refusal = refusal_record;
        }
        Ok(RetainedCandidate {
            operation_id: candidate.operation_id.clone(),
            input_hash: candidate
                .input_hash()
                .map_err(|_| JournalError::InvalidInput)?,
            refusal: self.refusal.as_ref().map(|record| record.refusal),
        })
    }

    /// Retain a descriptive historical claim only. This method permanently
    /// demotes this instance and returns no checkpoint or runtime permission.
    pub(crate) fn retain_observed_claim(
        &mut self,
        tenant_id: &str,
        history: &crate::control::kubernetes_admission::KubernetesCreateClaimResponseV1,
    ) -> JournalResult<()> {
        self.disk.revalidate()?;
        records::require(
            tenant_id == self.prepared.tenant_id
                && history.version == 1
                && history.disposition
                    == crate::control::kubernetes_admission::KubernetesCreateDispositionV1::ObserveOnly,
        )?;
        let record = EmissionRecord {
            version: 1,
            prepared_sha256: self.disk.commitment("prepared.json")?.into(),
            claim: history
                .snapshot
                .claim
                .clone()
                .ok_or(JournalError::InvalidInput)?,
            receipt: history.receipt.clone(),
            snapshot: history.snapshot.clone(),
        };
        record.validate(&self.prepared, self.disk.commitment("prepared.json")?)?;
        let bytes = records::encode(&record, 16 * 1024)?;
        self.disk.protection.screen_record(&bytes)?;
        // Once coherent history has been accepted, even a failed local write
        // cannot resurrect an old live checkpoint. No DTO reconstructs freshness.
        self.recovered = true;
        self.disk.append("emission.json", &bytes)?;
        self.emission = Some(record);
        Ok(())
    }

    /// A remote result confirmation is descriptive and grants no emission.
    pub(crate) fn acknowledge_remote_result(
        &mut self,
        confirmed: ConfirmedHeldCreateResultAck,
    ) -> JournalResult<Option<KubernetesPodHandle>> {
        // The HTTP proof binds transport/request provenance. This routine also
        // requires the whole locally predicted snapshot before persisting ACK.
        self.acknowledge_result_record(
            confirmed.tenant_id(),
            confirmed.result(),
            confirmed.receipt(),
            confirmed.snapshot(),
        )
    }

    pub(crate) fn acknowledge_result(
        &mut self,
        confirmed: ConfirmedKubernetesCreateResult,
    ) -> JournalResult<Option<KubernetesPodHandle>> {
        self.acknowledge_result_record(
            confirmed.tenant_id(),
            confirmed.result(),
            confirmed.receipt(),
            confirmed.snapshot(),
        )
    }

    fn acknowledge_result_record(
        &mut self,
        tenant_id: &str,
        result: &KubernetesCreateResultV1,
        receipt: &KubernetesCreateReceiptV1,
        snapshot: &KubernetesCreateSnapshotV1,
    ) -> JournalResult<Option<KubernetesPodHandle>> {
        self.disk.revalidate()?;
        let candidate = self.candidate.as_ref().ok_or(JournalError::InvalidInput)?;
        let emission = self.emission.as_ref().ok_or(JournalError::Corrupt)?;
        records::require(
            tenant_id == self.prepared.tenant_id
                && result == candidate
                && *snapshot == records::result_snapshot(emission, candidate)?,
        )?;
        let record = AckRecord {
            version: 1,
            prepared_sha256: self.disk.commitment("prepared.json")?.into(),
            candidate_sha256: self.disk.commitment("candidate.json")?.into(),
            receipt: receipt.clone(),
        };
        records::validate_ack(
            &self.prepared,
            self.disk.commitment("prepared.json")?,
            emission,
            candidate,
            self.disk.commitment("candidate.json")?,
            &record,
        )?;
        let bytes = records::encode(&record, 16 * 1024)?;
        self.disk.append("ack.json", &bytes)?;
        self.ack = Some(record);
        // Only the opaque exact central ACK and the now-durable local ACK can
        // publish this descriptive inspection handle. Reopen alone never does.
        match &candidate.outcome {
            crate::control::kubernetes_admission::KubernetesCreateOutcomeV1::CreatedHeld {
                observation,
                admitted_configuration_sha256,
                ..
            } => KubernetesPodHandle::from_control_checkpoint(
                &self.prepared.intent.authority,
                uuid::Uuid::parse_str(&self.prepared.intent.session_id)
                    .map_err(|_| JournalError::Corrupt)?,
                uuid::Uuid::parse_str(&self.prepared.intent.create_id)
                    .map_err(|_| JournalError::Corrupt)?,
                &observation.metadata.name,
                &observation.metadata.uid,
                admitted_configuration_sha256,
            )
            .map(Some)
            .map_err(|_| JournalError::Corrupt),
            crate::control::kubernetes_admission::KubernetesCreateOutcomeV1::OwnershipUnknown {
                ..
            } => Ok(None),
        }
    }

    pub(crate) fn recover(
        root: Directory,
        protection: AdmissionJournalProtection,
    ) -> JournalResult<Option<RecoveredHeldCreate>> {
        let Some(mut disk) = fs::Disk::acquire(root, protection, false)? else {
            return Ok(None);
        };
        let names = fs::inventory(&disk.root)?;
        let mut payloads = std::collections::BTreeMap::new();
        for name in &names {
            if name == ".writer.lock" {
                continue;
            }
            let file = disk.read(name)?.ok_or(JournalError::Corrupt)?;
            disk.protection.screen_record(file.bytes())?;
            disk.remember_file(name, &file)?;
            payloads.insert(name.clone(), file.bytes().to_vec());
        }
        let get = |name: &str| {
            payloads
                .get(name)
                .map(Vec::as_slice)
                .ok_or(JournalError::Corrupt)
        };
        let prepared: PreparedRecord = records::decode(get("prepared.json")?, 32 * 1024)?;
        prepared
            .validate(get("pod.json")?)
            .map_err(|_| JournalError::Corrupt)?;
        let emission: Option<EmissionRecord> = payloads
            .get("emission.json")
            .map(|bytes| records::decode(bytes, 16 * 1024))
            .transpose()?;
        if let Some(emission) = &emission {
            emission
                .validate(&prepared, disk.commitment("prepared.json")?)
                .map_err(|_| JournalError::Corrupt)?;
        }
        let candidate: Option<KubernetesCreateResultV1> = payloads
            .get("candidate.json")
            .map(|bytes| records::decode(bytes, 64 * 1024))
            .transpose()?;
        if let Some(candidate) = &candidate {
            records::validate_candidate(
                &prepared,
                emission.as_ref().ok_or(JournalError::Corrupt)?,
                candidate,
            )
            .map_err(|_| JournalError::Corrupt)?;
        }
        let refusal: Option<RefusalRecord> = payloads
            .get("refusal.json")
            .map(|bytes| records::decode(bytes, 1024))
            .transpose()?;
        if let Some(refusal) = &refusal {
            if candidate.is_none()
                || refusal.version != 1
                || refusal.candidate_sha256 != disk.commitment("candidate.json")?
            {
                return Err(JournalError::Corrupt);
            }
        }
        let ack: Option<AckRecord> = payloads
            .get("ack.json")
            .map(|bytes| records::decode(bytes, 16 * 1024))
            .transpose()?;
        if let Some(ack) = &ack {
            records::validate_ack(
                &prepared,
                disk.commitment("prepared.json")?,
                emission.as_ref().ok_or(JournalError::Corrupt)?,
                candidate.as_ref().ok_or(JournalError::Corrupt)?,
                disk.commitment("candidate.json")?,
                ack,
            )
            .map_err(|_| JournalError::Corrupt)?;
        }
        disk.resynchronize()?;
        let phase = if ack.is_some() {
            RecoveryPhase::ResultAcknowledged
        } else if candidate.is_some() {
            RecoveryPhase::CandidateRetained
        } else if emission.is_some() {
            RecoveryPhase::ObserveOnly
        } else {
            RecoveryPhase::Prepared
        };
        Ok(Some(RecoveredHeldCreate {
            journal: Self {
                disk,
                prepared,
                emission,
                candidate,
                refusal,
                ack,
                recovered: true,
            },
            phase,
        }))
    }

    pub(crate) fn candidate(&self) -> Option<&KubernetesCreateResultV1> {
        self.candidate.as_ref()
    }
    pub(crate) fn candidate_refusal(&self) -> Option<CandidateRefusal> {
        self.refusal.as_ref().map(|record| record.refusal)
    }

    pub(crate) fn tenant_id(&self) -> &str {
        &self.prepared.tenant_id
    }
    pub(crate) fn intent(&self) -> &KubernetesCreateIntentV1 {
        &self.prepared.intent
    }
    pub(crate) fn target(&self) -> &crate::control::kubernetes_admission::KubernetesCreateTargetV1 {
        &self.prepared.target
    }
    pub(crate) fn claim_request(&self) -> &KubernetesCreateClaimV1 {
        &self.prepared.claim_request
    }
    pub(crate) fn observation_only(&self) -> bool {
        self.recovered
    }
    pub(crate) fn revalidate_retained(&self) -> JournalResult<()> {
        self.disk.revalidate()
    }
    pub(crate) fn revalidate_emission(&self, checkpoint: &EmissionCheckpoint) -> JournalResult<()> {
        self.disk.revalidate()?;
        records::require(
            !self.recovered
                && checkpoint.tenant_id == self.prepared.tenant_id
                && checkpoint.create_id == self.prepared.intent.create_id
                && checkpoint.intent_hash == self.prepared.claim_request.intent_hash
                && checkpoint.claim_operation_id == self.prepared.claim_request.operation_id
                && checkpoint.create_epoch == self.prepared.claim_request.expected_create_epoch
                && checkpoint.request_sha256 == self.prepared.target.request_sha256
                && checkpoint.emission_sha256 == self.disk.commitment("emission.json")?
                && self.candidate.is_none()
                && self.ack.is_none(),
        )
    }
}

#[cfg(test)]
#[path = "admission_journal_tests.rs"]
mod tests;
