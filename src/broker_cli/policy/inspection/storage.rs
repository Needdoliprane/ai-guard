use agentsandbox::{
    control::egress_policy::{EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1},
    control_fs::{
        Directory, PrivateFile, absolute_path,
        operation::{DurableOperationDirectory, OperationRecord},
    },
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::super::storage::{fs_error, origin};

fn report_origin(value: &str) -> Result<String> {
    // Reports retain the canonical origin without a root-path slash. Existing
    // A1 operation metadata keeps its established Url::to_string form.
    let canonical = origin(value)?;
    canonical
        .strip_suffix('/')
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("egress_policy_invalid_origin"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u32,
    kind: Kind,
    gateway: String,
    request_sha256: String,
    observation_sha256: String,
}

#[derive(Serialize, Deserialize)]
enum Kind {
    #[serde(rename = "policy_inspection")]
    PolicyInspection,
}

fn bytes(directory: &DurableOperationDirectory, record: OperationRecord) -> Result<&[u8]> {
    directory
        .record(record)
        .map(PrivateFile::bytes)
        .ok_or_else(|| anyhow!("egress_policy_incomplete_inspection"))
}

fn encode(metadata: &Metadata) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(metadata)
        .map_err(|_| anyhow!("egress_policy_invalid_inspection_metadata"))?;
    ensure!(bytes.len() <= 4096, "egress_policy_invalid_inspection_metadata");
    Ok(bytes)
}

/// Read-only wrapper: no state publication, pending recovery, receipt or Drop.
/// Its hashes correlate local bytes; the report carries no remote authority.
pub(in crate::broker_cli::policy) struct Report {
    directory: DurableOperationDirectory,
    pub(in crate::broker_cli::policy) observation: EgressPolicyInspectionV1,
    metadata: Metadata,
}

impl Report {
    pub(super) fn prepare(
        gateway: &str,
        request: &EgressPolicyInspectionRequestV1,
        observation: &EgressPolicyInspectionV1,
        output: &Path,
        revalidate_source: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        observation
            .validate_for(request)
            .map_err(|_| anyhow!("egress_policy_invalid_response"))?;
        let request_bytes = request
            .canonical_bytes()
            .map_err(|_| anyhow!("egress_policy_invalid_request"))?;
        let observation_bytes = observation
            .canonical_bytes()
            .map_err(|_| anyhow!("egress_policy_invalid_response"))?;
        ensure!(
            request_bytes.len() <= 65_536 && observation_bytes.len() <= 4096,
            "egress_policy_invalid_response"
        );
        let metadata = Metadata {
            version: 1,
            kind: Kind::PolicyInspection,
            gateway: report_origin(gateway)?,
            request_sha256: hex::encode(Sha256::digest(&request_bytes)),
            observation_sha256: hex::encode(Sha256::digest(&observation_bytes)),
        };
        let metadata_bytes = encode(&metadata)?;
        let output = absolute_path(output).map_err(|_| anyhow!("egress_policy_unsafe_output"))?;
        let parent_path = output
            .parent()
            .ok_or_else(|| anyhow!("egress_policy_unsafe_output"))?;
        let name = output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("egress_policy_unsafe_output"))?;
        let parent = Directory::open_existing(parent_path)
            .map_err(|_| anyhow!("egress_policy_unsafe_output_parent"))?;
        revalidate_source()?;
        fs_error(DurableOperationDirectory::prepare_new(
            &parent,
            name,
            &request_bytes,
            &metadata_bytes,
            &observation_bytes,
        ))?;
        // A later refusal preserves the published report as evidence and does
        // not claim that publication did not happen.
        revalidate_source()?;
        let report = Self::open(&output)?;
        revalidate_source()?;
        report.revalidate()?;
        Ok(report)
    }

    pub(in crate::broker_cli::policy) fn open(path: &Path) -> Result<Self> {
        let directory = fs_error(DurableOperationDirectory::open_existing(path))?;
        ensure!(
            directory.record(OperationRecord::PendingState).is_none()
                && directory.record(OperationRecord::Receipt).is_none(),
            "egress_policy_invalid_inspection_inventory"
        );
        let metadata: Metadata = serde_json::from_slice(bytes(&directory, OperationRecord::Metadata)?)
            .map_err(|_| anyhow!("egress_policy_invalid_inspection_metadata"))?;
        ensure!(
            metadata.version == 1
                && report_origin(&metadata.gateway)? == metadata.gateway
                && encode(&metadata)?.as_slice() == bytes(&directory, OperationRecord::Metadata)?,
            "egress_policy_invalid_inspection_metadata"
        );
        let request_bytes = bytes(&directory, OperationRecord::Request)?;
        let request = EgressPolicyInspectionRequestV1::from_json(request_bytes)
            .map_err(|_| anyhow!("egress_policy_invalid_inspection_request"))?;
        ensure!(
            request.canonical_bytes().map_err(|_| anyhow!("egress_policy_invalid_inspection_request"))?.as_slice() == request_bytes,
            "egress_policy_noncanonical_inspection_request"
        );
        let observation_bytes = bytes(&directory, OperationRecord::State)?;
        let observation = EgressPolicyInspectionV1::from_canonical_bytes(observation_bytes)
            .map_err(|_| anyhow!("egress_policy_invalid_inspection_observation"))?;
        observation
            .validate_for(&request)
            .map_err(|_| anyhow!("egress_policy_inspection_mismatch"))?;
        ensure!(
            metadata.request_sha256 == hex::encode(Sha256::digest(request_bytes))
                && metadata.observation_sha256 == hex::encode(Sha256::digest(observation_bytes)),
            "egress_policy_inspection_hash_mismatch"
        );
        let report = Self { directory, observation, metadata };
        report.revalidate()?;
        Ok(report)
    }

    pub(in crate::broker_cli::policy) fn gateway(&self) -> &str {
        &self.metadata.gateway
    }

    pub(in crate::broker_cli::policy) fn require_output_parent_outside(
        &self,
        parent: &Directory,
    ) -> Result<()> {
        // The facade compares physical identities from both retained nofollow
        // ancestry chains, including case/Unicode aliases on the filesystem.
        ensure!(
            !fs_error(self.directory.contains_directory(parent))?,
            "egress_policy_output_overlaps_inspection"
        );
        Ok(())
    }

    pub(in crate::broker_cli::policy) fn revalidate(&self) -> Result<()> {
        fs_error(self.directory.revalidate())?;
        ensure!(
            self.directory.record(OperationRecord::PendingState).is_none()
                && self.directory.record(OperationRecord::Receipt).is_none(),
            "egress_policy_invalid_inspection_inventory"
        );
        Ok(())
    }
}
