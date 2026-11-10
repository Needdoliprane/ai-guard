//! Authenticated transport metadata, never execution authority or runtime facts.
use super::*;

pub const MAX_BROKER_CHECKPOINT_JOBS: usize = 100;
pub const MAX_BROKER_CHECKPOINT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerJobCheckpointV1 {
    pub job_id: String,
    pub revision: u64,
    pub checkpoint_hash: String,
}

/// Captured from the trusted broker's own confirmed heads. References remain
/// declared until independently mediated; an empty list is not zero activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointSnapshotV1 {
    pub version: u32,
    pub authority: BrokerAuthorityV1,
    pub jobs: Vec<BrokerJobCheckpointV1>,
    pub has_more: bool,
}
impl BrokerCheckpointSnapshotV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.jobs.len() <= MAX_BROKER_CHECKPOINT_JOBS)?;
        self.authority.validate()?;
        let mut previous: Option<&str> = None;
        for job in &self.jobs {
            registry_uuid(&job.job_id)?;
            revision(job.revision)?;
            require(
                job.checkpoint_hash.len() == 64
                    && job
                        .checkpoint_hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    && previous.is_none_or(|id| id < job.job_id.as_str()),
            )?;
            previous = Some(&job.job_id);
        }
        require(serde_json::to_vec(self)?.len() <= MAX_BROKER_CHECKPOINT_BYTES)
    }
}

/// Operation and sequence are fixed durably before sending. No tenant or
/// broker field can select a scope: both come from the verified Broker proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointV1 {
    pub version: u32,
    pub operation_id: String,
    pub incarnation_id: String,
    pub previous_sequence: u64,
    pub sequence: u64,
    pub snapshot: BrokerCheckpointSnapshotV1,
}
impl BrokerCheckpointV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1)?;
        registry_uuid(&self.operation_id)?;
        registry_uuid(&self.incarnation_id)?;
        revision(self.previous_sequence)?;
        revision(self.sequence)?;
        require(self.previous_sequence.checked_add(1) == Some(self.sequence))?;
        self.snapshot.validate()?;
        require(serde_json::to_vec(self)?.len() <= MAX_BROKER_CHECKPOINT_BYTES)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerCheckpointCoverageV1 {
    DeclaredPartial,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerCheckpointFreshnessV1 {
    Current,
    Stale,
    Historical,
    ClockUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointResultV1 {
    pub checkpoint: BrokerCheckpointV1,
    pub receipt: BrokerMutationReceiptV1,
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointObservationV1 {
    pub checkpoint: BrokerCheckpointV1,
    pub receipt: BrokerMutationReceiptV1,
    pub received_at: String,
    pub lease_expires_at: String,
    pub in_current_grant: bool,
    pub freshness: BrokerCheckpointFreshnessV1,
    pub coverage: BrokerCheckpointCoverageV1,
    pub publication_provenance: BrokerClaimProvenanceV1,
    pub reception_provenance: BrokerReceptionProvenanceV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerCheckpointViewV1 {
    pub version: u32,
    pub broker: BrokerViewV1,
    pub checkpoint: Option<BrokerCheckpointObservationV1>,
}
