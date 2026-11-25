//! Broker-private data for bootstrap opening reconciliation and release intent.
//! A decoded record is historical evidence. The runtime still needs a live
//! opening, its private transport, and the current stop/deadline guard.

use crate::{
    broker::{egress_owner::channel::OpenedChannel, journal::canonical},
    control::{
        MutationResultV1, NetworkV1, RuntimeStatusV1, WorkerStateV1, WorkerV1,
        egress_decision::{
            EgressBindingV1, EgressChannelOpeningDispositionV1, EgressChannelOpeningReceiptV1,
            EgressChannelOpeningRequestV1,
        },
    },
    egress::ids::{DigestV1, IdV1},
    runtime::{ContainerObservation, OwnedContainerSnapshot, RuntimeAuthority},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EgressReleaseTargetV1 {
    tenant_id: String,
    operation_id: IdV1,
    session_id: IdV1,
    worker_id: IdV1,
    attempt_id: IdV1,
    authority_id: IdV1,
    authority_fingerprint: DigestV1,
    generation: u64,
    container_id: DigestV1,
    started_at: DateTime<Utc>,
    restart_count: u64,
    configuration_fingerprint: DigestV1,
    configuration_fingerprint_version: u32,
}

impl EgressReleaseTargetV1 {
    pub(crate) fn from_observation(
        tenant_id: &str,
        generation: u64,
        worker: &WorkerV1,
        authority: &RuntimeAuthority,
        owned: &OwnedContainerSnapshot,
        observation: &ContainerObservation,
    ) -> Result<Self> {
        observation.validate_identity_allocation(&owned.observation)?;
        ensure!(
            observation.process.status.as_deref() == Some("running")
                && observation.process.running == Some(true),
            "bootstrap runtime is not observed running"
        );
        let value = Self {
            tenant_id: tenant_id.into(),
            operation_id: IdV1::parse(&worker.admission_operation_id)?,
            session_id: IdV1::parse(&worker.launch.session_id)?,
            worker_id: IdV1::parse(&worker.worker_id)?,
            attempt_id: IdV1::parse(&worker.attempt.attempt_id)?,
            authority_id: IdV1::new(authority.authority_id)?,
            authority_fingerprint: DigestV1::parse(&authority.fingerprint())?,
            generation,
            container_id: DigestV1::parse(owned.handle.id())?,
            started_at: observation
                .process
                .started_at
                .as_ref()
                .context("bootstrap start time is missing")?
                .to_utc(),
            restart_count: observation
                .restart_count
                .context("bootstrap restart count is missing")?,
            configuration_fingerprint: DigestV1::parse(
                observation
                    .configuration_fingerprint
                    .as_deref()
                    .context("bootstrap confinement digest is missing")?,
            )?,
            configuration_fingerprint_version: observation
                .configuration_fingerprint_version
                .unwrap_or(1),
        };
        value.validate_retained(tenant_id, worker, authority, owned)?;
        Ok(value)
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(
            !self.tenant_id.is_empty()
                && self.tenant_id.len() <= 256
                && self.generation != u64::MAX
                && self.restart_count == 0
                && matches!(self.configuration_fingerprint_version, 1..=3),
            "bootstrap release target is invalid"
        );
        Ok(())
    }

    /// Channel identity before the gateway acknowledges Running.
    pub(crate) fn binding(&self, channel_id: IdV1, epoch: u64) -> Result<EgressBindingV1> {
        self.validate_shape()?;
        ensure!(epoch > 0, "bootstrap channel epoch must be positive");
        let mut framed = b"ai-guard:egress-release-target:v1\0".to_vec();
        framed.extend(canonical(&(self, &channel_id, epoch))?);
        Ok(EgressBindingV1 {
            session_id: self.session_id.as_str().into(),
            worker_id: self.worker_id.as_str().into(),
            attempt_id: self.attempt_id.as_str().into(),
            channel_id,
            epoch,
            context_digest: DigestV1::of(&framed),
        })
    }

    fn validate_retained(
        &self,
        tenant_id: &str,
        worker: &WorkerV1,
        authority: &RuntimeAuthority,
        owned: &OwnedContainerSnapshot,
    ) -> Result<()> {
        self.validate_shape()?;
        ensure!(
            self.tenant_id == tenant_id
                && self.operation_id.as_str() == worker.admission_operation_id
                && self.session_id.as_str() == worker.launch.session_id
                && self.worker_id.as_str() == worker.worker_id
                && self.attempt_id.as_str() == worker.attempt.attempt_id
                && self.authority_id.as_str() == worker.launch.authority_id
                && self.authority_id == IdV1::new(authority.authority_id)?
                && self.authority_fingerprint.as_str() == authority.fingerprint()
                && worker.launch.egress.is_some()
                && worker.launch.profile == "observe"
                && worker.launch.network == NetworkV1::None
                && !worker.launch.security_preset.is_interactive()
                && worker.attempt.container_id.as_deref() == Some(self.container_id.as_str())
                && owned.handle.id() == self.container_id.as_str()
                && owned.handle.session_id().to_string() == self.session_id.as_str()
                && owned.handle.operation_id().to_string() == self.operation_id.as_str()
                && owned.handle.authority_fingerprint() == self.authority_fingerprint.as_str()
                && owned.handle.fingerprint_version() == self.configuration_fingerprint_version
                && owned.observation.configuration_fingerprint.as_deref()
                    == Some(self.configuration_fingerprint.as_str())
                && owned
                    .observation
                    .configuration_fingerprint_version
                    .unwrap_or(1)
                    == self.configuration_fingerprint_version
                && owned.observation.restart_count == Some(self.restart_count),
            "bootstrap release target differs from the retained attempt"
        );
        Ok(())
    }

    fn matches_running(&self, worker: &WorkerV1) -> bool {
        worker.state == WorkerStateV1::Running
            && worker.quota_held
            && !worker.allocation_inconsistent
            && worker.last_observation.as_ref().is_some_and(|observation| {
                observation.inspection.container_id == self.container_id.as_str()
                    && observation.inspection.authority_id == self.authority_id.as_str()
                    && observation.status == RuntimeStatusV1::Running
                    && observation.running == Some(true)
                    && observation.restart_count == Some(0)
                    && observation
                        .started_at
                        .as_deref()
                        .and_then(|start| DateTime::parse_from_rfc3339(start).ok())
                        .is_some_and(|start| start.to_utc() == self.started_at)
            })
    }

    fn matches_running_revision(&self, worker: &WorkerV1, revision: u64) -> bool {
        worker.revision == revision
            && self.worker_id.as_str() == worker.worker_id
            && self.attempt_id.as_str() == worker.attempt.attempt_id
            && self.matches_running(worker)
    }

    fn matches_running_ack(&self, ack: &MutationResultV1, revision: u64, audit_seq: i64) -> bool {
        ack.receipt.audit_seq == audit_seq
            && ack.receipt.revision == Some(revision)
            && self.matches_running_revision(&ack.worker, revision)
    }
}

/// Reconciliation identity retained before the POST. This can be decoded after
/// a crash and carries neither a channel owner nor permission to send anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedEgressOpeningV1 {
    version: u32,
    target: EgressReleaseTargetV1,
    #[serde(deserialize_with = "crate::control::egress_decision::deserialize_object")]
    request: EgressChannelOpeningRequestV1,
    worker_revision: u64,
    running_audit_seq: i64,
}

impl RetainedEgressOpeningV1 {
    pub(crate) fn from_running(
        target: EgressReleaseTargetV1,
        request: EgressChannelOpeningRequestV1,
        running: &MutationResultV1,
    ) -> Result<Self> {
        let value = Self {
            version: 1,
            target,
            request,
            worker_revision: running.worker.revision,
            running_audit_seq: running.receipt.audit_seq,
        };
        value.validate_shape()?;
        ensure!(
            value.matches_running_ack(running),
            "bootstrap Running receipt differs"
        );
        Ok(value)
    }

    pub(crate) fn request(&self) -> &EgressChannelOpeningRequestV1 {
        &self.request
    }

    pub(crate) fn generation(&self) -> u64 {
        self.target.generation
    }

    pub(crate) fn matches_running(&self, worker: &WorkerV1) -> bool {
        self.target
            .matches_running_revision(worker, self.worker_revision)
    }

    pub(crate) fn matches_running_ack(&self, running: &MutationResultV1) -> bool {
        self.target
            .matches_running_ack(running, self.worker_revision, self.running_audit_seq)
    }

    fn validate_shape(&self) -> Result<()> {
        let binding = &self.request.binding;
        ensure!(
            self.version == 1
                && self.request.version == 1
                && self.worker_revision > 0
                && self.running_audit_seq > 0
                && *binding
                    == self
                        .target
                        .binding(binding.channel_id.clone(), binding.epoch)?,
            "bootstrap opening reconciliation identity is invalid"
        );
        Ok(())
    }

    pub(crate) fn validate_retained(
        &self,
        tenant_id: &str,
        worker: &WorkerV1,
        authority: &RuntimeAuthority,
        owned: &OwnedContainerSnapshot,
    ) -> Result<()> {
        self.validate_shape()?;
        ensure!(
            worker.revision >= self.worker_revision,
            "bootstrap opening worker revision regressed"
        );
        self.target
            .validate_retained(tenant_id, worker, authority, owned)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EgressReleaseIntentV1 {
    version: u32,
    target: EgressReleaseTargetV1,
    opening: EgressChannelOpeningReceiptV1,
    worker_revision: u64,
    running_audit_seq: i64,
}

impl EgressReleaseIntentV1 {
    /// Freeze only an opening from this live exchange. Recovery can read the
    /// persisted intent but cannot recreate an opened channel from its bytes.
    pub(crate) fn from_opened<IO>(
        target: EgressReleaseTargetV1,
        opened: &OpenedChannel<IO>,
        running: &MutationResultV1,
    ) -> Result<Self> {
        let value = Self {
            version: 1,
            target,
            opening: opened.receipt().clone(),
            worker_revision: running.worker.revision,
            running_audit_seq: running.receipt.audit_seq,
        };
        value.validate_shape()?;
        ensure!(
            value.matches_running_ack(running),
            "bootstrap Running receipt differs"
        );
        Ok(value)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.target.generation
    }

    pub(crate) fn matches_running(&self, worker: &WorkerV1) -> bool {
        self.target
            .matches_running_revision(worker, self.worker_revision)
    }

    pub(crate) fn matches_running_ack(&self, running: &MutationResultV1) -> bool {
        self.target
            .matches_running_ack(running, self.worker_revision, self.running_audit_seq)
    }

    pub(crate) fn matches_retained_opening(&self, retained: &RetainedEgressOpeningV1) -> bool {
        let request = retained.request();
        self.target == retained.target
            && self.worker_revision == retained.worker_revision
            && self.running_audit_seq == retained.running_audit_seq
            && self.opening.version == request.version
            && self.opening.observation_id == request.observation_id
            && self.opening.binding == request.binding
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.worker_revision > 0
                && self.opening.version == 1
                && self.opening.disposition == EgressChannelOpeningDispositionV1::Fresh
                && self.running_audit_seq > 0
                && self.opening.receipt.audit_seq > self.running_audit_seq
                && self.opening.binding
                    == self.target.binding(
                        self.opening.binding.channel_id.clone(),
                        self.opening.binding.epoch,
                    )?,
            "bootstrap release opening is invalid"
        );
        DigestV1::parse(&self.opening.receipt.audit_hash)?;
        Ok(())
    }

    pub(crate) fn validate_retained(
        &self,
        tenant_id: &str,
        worker: &WorkerV1,
        authority: &RuntimeAuthority,
        owned: &OwnedContainerSnapshot,
    ) -> Result<()> {
        self.validate_shape()?;
        ensure!(
            worker.revision >= self.worker_revision,
            "bootstrap release worker revision regressed"
        );
        self.target
            .validate_retained(tenant_id, worker, authority, owned)
    }
}

#[cfg(test)]
#[path = "egress_release_tests.rs"]
mod tests;
