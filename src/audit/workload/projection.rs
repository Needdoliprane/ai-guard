//! State transitions for the workload binding projection.
#[cfg(feature = "mysql-storage")]
use crate::workload::{WorkloadDigestOwnerV1, workload_authorized};
use crate::{
    broker_registry::{format_time, parse_time},
    control::{WorkerStateV1, WorkerV1},
    relay::RelayEndpointV1,
    workload::{
        WorkloadBinding, WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadFailureV1,
        WorkloadSubjectV1, workload_conflict, workload_valid,
    },
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Authorization/expiry use millisecond UTC, matching their durable records.
/// This does not claim detection of a cross-request rollback within one ms.
pub(crate) fn canonical_control_time(at: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&format_time(at))
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| WorkloadFailureV1::ClockRegression.into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Genesis {
    pub(crate) version: u32,
    pub(crate) tenant_id: String,
    pub(crate) control_namespace_uuid: String,
    pub(crate) previous_seq: i64,
    pub(crate) previous_hash: String,
}

impl Genesis {
    pub(crate) fn matches_prefix(
        &self,
        tenant: &str,
        namespace: &str,
        seq: i64,
        previous_hash: &str,
    ) -> bool {
        self.version == 1
            && self.tenant_id == tenant
            && self.control_namespace_uuid == namespace
            && seq.checked_sub(1) == Some(self.previous_seq)
            && self.previous_hash == previous_hash
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Change {
    Issue { input: WorkloadBindingIssueV1 },
    Revoke { input: WorkloadBindingRevocationV1 },
}
impl Change {
    pub(super) fn operation_id(&self) -> &str {
        match self {
            Self::Issue { input } => &input.operation_id,
            Self::Revoke { input } => &input.operation_id,
        }
    }
    pub(super) fn credential_id(&self) -> &str {
        match self {
            Self::Issue { input } => &input.credential_id,
            Self::Revoke { input } => &input.credential_id,
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Issue { .. } => "control.workload.issue",
            Self::Revoke { .. } => "control.workload.revoke",
        }
    }
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Issue { input } => input.validate(),
            Self::Revoke { input } => input.validate(),
        }
    }
    pub(crate) fn input_hash(&self) -> anyhow::Result<String> {
        crate::control::content_hash("ai-guard:workload-binding-input:v1", self)
    }

    pub(crate) fn worker_issue_endpoint(&self) -> Option<RelayEndpointV1> {
        let Self::Issue { input } = self else {
            return None;
        };
        let WorkloadSubjectV1::Worker {
            worker_id,
            attempt_id,
            generation,
        } = &input.subject
        else {
            return None;
        };
        Some(RelayEndpointV1 {
            worker_id: worker_id.clone(),
            attempt_id: attempt_id.clone(),
            generation: *generation,
        })
    }
}

pub(crate) fn worker_matches(worker: &WorkerV1, endpoint: &RelayEndpointV1) -> bool {
    worker.worker_id == endpoint.worker_id
        && worker.attempt.attempt_id == endpoint.attempt_id
        && worker.quota_held
        && !matches!(
            worker.state,
            WorkerStateV1::Terminal | WorkerStateV1::Cancelled
        )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) version: u32,
    pub(crate) input_hash: String,
    pub(crate) received_at: String,
    pub(crate) change: Change,
}
#[derive(Default)]
pub(crate) struct Projection {
    pub(super) bindings: BTreeMap<String, (WorkloadBinding, i64)>,
    pub(super) history: BTreeMap<(String, u64), (WorkloadBinding, i64)>,
    pub(super) digests: BTreeMap<String, String>,
    pub(super) operations: BTreeMap<String, (String, i64, String, u64)>,
    pub(super) endpoints: BTreeMap<String, (RelayEndpointV1, i64)>,
    pub(super) subject_counts: BTreeMap<String, u64>,
}

pub(crate) enum ProjectionCell {
    Integer(i64),
    Text(String),
}

pub(crate) type ProjectionRows = BTreeMap<&'static str, Vec<Vec<ProjectionCell>>>;

impl Projection {
    #[cfg(feature = "mysql-storage")]
    pub(crate) fn replay_sequence(&self, change: &Change) -> anyhow::Result<Option<i64>> {
        let Some((hash, sequence, _, _)) = self.operations.get(change.operation_id()) else {
            return Ok(None);
        };
        workload_conflict(hash == &change.input_hash()?)?;
        Ok(Some(*sequence))
    }

    #[cfg(feature = "mysql-storage")]
    pub(crate) fn has_historical_digest(&self, digest: &[u8; 32]) -> bool {
        self.digests.contains_key(&hex::encode(digest))
    }

    #[cfg(feature = "mysql-storage")]
    pub(crate) fn issue_index_insertions(
        &self,
        digest: &str,
        endpoint: Option<RelayEndpointV1>,
    ) -> (bool, Option<RelayEndpointV1>) {
        (
            !self.digests.contains_key(digest),
            endpoint.filter(|endpoint| !self.endpoints.contains_key(&endpoint.worker_id)),
        )
    }

    #[cfg(feature = "mysql-storage")]
    pub(crate) fn historical_digest_owner(
        &self,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<WorkloadDigestOwnerV1>> {
        let Some(credential_id) = self.digests.get(&hex::encode(digest)) else {
            return Ok(None);
        };
        let (binding, _) = self
            .bindings
            .get(credential_id)
            .ok_or(WorkloadFailureV1::Unauthorized)?;
        workload_authorized(&binding.credential_id == credential_id)?;
        Ok(Some(WorkloadDigestOwnerV1 {
            credential_id: credential_id.clone(),
            subject: binding.subject.clone(),
        }))
    }

    /// The caller checks the control-time floor and any worker state first.
    /// Replay also checks the audit envelope. A failed apply can leave partial state.
    pub(crate) fn apply(&mut self, record: &Record, seq: i64) -> anyhow::Result<WorkloadBinding> {
        record.change.validate()?;
        workload_valid(record.version == 1 && record.input_hash == record.change.input_hash()?)?;
        workload_conflict(!self.operations.contains_key(record.change.operation_id()))?;
        let now = parse_time(&record.received_at)?;
        let previous = self
            .bindings
            .get(record.change.credential_id())
            .map(|(binding, _)| binding);
        if let Some(previous) = previous {
            if now < parse_time(&previous.updated_at)? {
                return Err(WorkloadFailureV1::ClockRegression.into());
            }
        }
        let binding = match &record.change {
            Change::Issue { input } => {
                let expires_at = canonical_control_time(parse_time(&input.expires_at)?)?;
                workload_valid(expires_at > now)?;
                workload_conflict(
                    input.expected_revision == previous.map_or(0, |binding| binding.revision),
                )?;
                if let Some(previous) = previous {
                    workload_conflict(previous.subject == input.subject)?;
                }
                if previous.is_none() {
                    let count = self.subject_counts.entry(input.subject.key()?).or_default();
                    workload_conflict(*count < 64)?;
                    *count += 1;
                }
                if let Some(owner) = self.digests.get(&input.sha256_digest) {
                    workload_conflict(owner == &input.credential_id)?;
                    // A historical digest is ownership evidence, not a fresh
                    // credential. Only renewal of the still-current active key
                    // may reuse it; rotation and revocation require a new key.
                    let current = previous.ok_or(WorkloadFailureV1::Conflict)?;
                    workload_conflict(
                        !current.revoked
                            && current.sha256_digest == input.sha256_digest
                            && now < parse_time(&current.expires_at)?,
                    )?;
                }
                if let WorkloadSubjectV1::Worker {
                    worker_id,
                    attempt_id,
                    generation,
                } = &input.subject
                {
                    let endpoint = RelayEndpointV1 {
                        worker_id: worker_id.clone(),
                        attempt_id: attempt_id.clone(),
                        generation: *generation,
                    };
                    if let Some((previous, _)) = self.endpoints.get(worker_id) {
                        workload_conflict(previous == &endpoint)?;
                    } else {
                        self.endpoints.insert(worker_id.clone(), (endpoint, seq));
                    }
                }
                let binding = WorkloadBinding {
                    version: 1,
                    credential_id: input.credential_id.clone(),
                    revision: crate::broker_registry::next_revision(input.expected_revision)?,
                    subject: input.subject.clone(),
                    sha256_digest: input.sha256_digest.clone(),
                    expires_at: format_time(expires_at),
                    revoked: false,
                    updated_at: record.received_at.clone(),
                };
                self.digests
                    .insert(input.sha256_digest.clone(), input.credential_id.clone());
                binding
            }
            Change::Revoke { input } => {
                let mut binding = previous.cloned().ok_or(WorkloadFailureV1::Conflict)?;
                workload_conflict(input.expected_revision == binding.revision && !binding.revoked)?;
                binding.revision = crate::broker_registry::next_revision(binding.revision)?;
                binding.revoked = true;
                binding.updated_at = record.received_at.clone();
                binding
            }
        };
        self.bindings
            .insert(binding.credential_id.clone(), (binding.clone(), seq));
        self.history.insert(
            (binding.credential_id.clone(), binding.revision),
            (binding.clone(), seq),
        );
        self.operations.insert(
            record.change.operation_id().into(),
            (
                record.input_hash.clone(),
                seq,
                binding.credential_id.clone(),
                binding.revision,
            ),
        );
        Ok(binding)
    }

    pub(crate) fn rows(
        &self,
        genesis_seq: i64,
        genesis_hash: &str,
    ) -> anyhow::Result<ProjectionRows> {
        use ProjectionCell::{Integer, Text};

        Ok(BTreeMap::from([
            (
                "workload_meta",
                vec![vec![
                    Integer(1),
                    Integer(genesis_seq),
                    Text(genesis_hash.to_owned()),
                ]],
            ),
            (
                "workload_bindings",
                self.bindings
                    .values()
                    .map(|(binding, seq)| {
                        Ok(vec![
                            Text(binding.credential_id.clone()),
                            Integer(i64::try_from(binding.revision)?),
                            Text(binding.sha256_digest.clone()),
                            Text(binding.subject.key()?),
                            Text(serde_json::to_string(binding)?),
                            Integer(*seq),
                        ])
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            (
                "workload_binding_history",
                self.history
                    .values()
                    .map(|(binding, seq)| {
                        Ok(vec![
                            Text(binding.credential_id.clone()),
                            Integer(i64::try_from(binding.revision)?),
                            Text(serde_json::to_string(binding)?),
                            Integer(*seq),
                        ])
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            (
                "workload_digests",
                self.digests
                    .iter()
                    .map(|(digest, id)| vec![Text(digest.clone()), Text(id.clone())])
                    .collect(),
            ),
            (
                "workload_operations",
                self.operations
                    .iter()
                    .map(|(op, (hash, seq, id, revision))| {
                        Ok(vec![
                            Text(op.clone()),
                            Text(hash.clone()),
                            Integer(*seq),
                            Text(id.clone()),
                            Integer(i64::try_from(*revision)?),
                        ])
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            (
                "workload_worker_endpoints",
                self.endpoints
                    .values()
                    .map(|(endpoint, seq)| {
                        Ok(vec![
                            Text(endpoint.worker_id.clone()),
                            Text(endpoint.attempt_id.clone()),
                            Integer(i64::try_from(endpoint.generation)?),
                            Integer(*seq),
                        ])
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
        ]))
    }
}
