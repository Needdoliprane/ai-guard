//! Broker-owned relay attachment. Authority comes from a verified job, its
//! original worker Arm, issued workload material and a fresh Core exec claim.
use super::{
    checkpoint::SupervisionOperationV1,
    clock::HostClock,
    enforcer::CancelGeneration,
    journal::{AcceptedJobV1, JournalScopeV1},
};
use crate::{
    control::{ArmSubjectV1, ArmV1, WorkerStateV1, WorkerV1},
    relay::RelayEndpointV1,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
pub(crate) mod journal;
mod registry;
pub(crate) use registry::{DispatchAuthority, RelayRegistry, VerifiedRelayExecAdmission};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RelayOwnerError {
    Unavailable,
    InvalidMaterial,
    Scope,
    Expired,
    Clock,
    Cancelled,
    Capacity,
    Unknown,
}
impl std::fmt::Display for RelayOwnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "worker relay dependency is unavailable",
            Self::InvalidMaterial => "worker relay issued material is incomplete or invalid",
            Self::Scope => "worker relay authority does not match the verified job",
            Self::Expired => "worker relay original worker deadline has expired",
            Self::Clock => "worker relay original boot or continuous clock is unavailable",
            Self::Cancelled => "worker relay attachment was cancelled",
            Self::Capacity => "worker relay shared capacity is exhausted",
            Self::Unknown => "worker relay outcome requires reconciliation",
        })
    }
}
impl std::error::Error for RelayOwnerError {}
type Result<T> = std::result::Result<T, RelayOwnerError>;

/// No Deserialize or public DTO constructor. Only the acknowledged Worker Arm
/// of a journal-verified job creates this, retaining its immutable clock anchor.
pub(crate) struct WorkerRelayDeadline {
    arm: ArmV1,
    clock: Arc<dyn HostClock>,
    cancel: CancelGeneration,
    generation: u64,
    last_ns: std::sync::Mutex<u128>,
}
impl WorkerRelayDeadline {
    fn from_job(
        job: &AcceptedJobV1,
        scope: &JournalScopeV1,
        clock: Arc<dyn HostClock>,
        cancel: CancelGeneration,
        generation: u64,
        terminal_recovery: bool,
    ) -> Result<(Arc<Self>, WorkerV1)> {
        if super::journal::canonical(job.scope()).map_err(|_| RelayOwnerError::Scope)?
            != super::journal::canonical(scope).map_err(|_| RelayOwnerError::Scope)?
        {
            return Err(RelayOwnerError::Scope);
        }
        let checkpoint = job.broker_checkpoint_v2().ok_or(RelayOwnerError::Scope)?;
        let operations = checkpoint.supervision().operations();
        let worker = operations
            .iter()
            .rev()
            .find_map(|operation| match operation {
                SupervisionOperationV1::WorkerMutation {
                    response: Some(result),
                    ..
                } => Some(result.worker.clone()),
                _ => None,
            })
            .ok_or(RelayOwnerError::Scope)?;
        if !(worker.state == WorkerStateV1::Running && worker.quota_held
            || terminal_recovery && worker.state == WorkerStateV1::Terminal && !worker.quota_held)
            || worker.allocation_inconsistent
        {
            return Err(RelayOwnerError::Scope);
        }
        let mut matching = operations.iter().filter_map(|operation| match operation {
            SupervisionOperationV1::Arm {
                request,
                response: Some(result),
                ..
            } if matches!(request.subject, ArmSubjectV1::Worker)
                && request.worker_id == worker.worker_id
                && request.attempt_id == worker.attempt.attempt_id =>
            {
                Some((request, result))
            }
            _ => None,
        });
        let (arm, answer) = matching.next().ok_or(RelayOwnerError::Scope)?;
        if matching.next().is_some()
            || answer.receipt.control_namespace_uuid != scope.control_namespace_uuid().to_string()
            || arm.intent_hash != job.intent_hash()
        {
            return Err(RelayOwnerError::Scope);
        }
        answer
            .receipt
            .validate_for_arm(arm)
            .map_err(|_| RelayOwnerError::Scope)?;
        let deadline = Arc::new(Self {
            last_ns: std::sync::Mutex::new(
                arm.clock
                    .continuous_ns()
                    .map_err(|_| RelayOwnerError::Clock)?,
            ),
            arm: arm.clone(),
            clock,
            cancel,
            generation,
        });
        Ok((deadline, worker))
    }
    pub(crate) fn check(&self) -> Result<()> {
        self.remaining().map(|_| ())
    }
    pub(crate) fn endpoint_matches(&self, endpoint: &RelayEndpointV1) -> bool {
        endpoint.generation == 1
            && endpoint.worker_id == self.arm.worker_id
            && endpoint.attempt_id == self.arm.attempt_id
    }
    fn remaining(&self) -> Result<Duration> {
        if self.cancel.current() != self.generation {
            return Err(RelayOwnerError::Cancelled);
        }
        let mut last = self.last_ns.lock().map_err(|_| RelayOwnerError::Clock)?;
        let sample = self.clock.sample().map_err(|_| RelayOwnerError::Clock)?;
        let before = self
            .arm
            .clock
            .continuous_ns()
            .map_err(|_| RelayOwnerError::Clock)?;
        let now = sample.continuous_ns().map_err(|_| RelayOwnerError::Clock)?;
        if sample.method != self.arm.clock.method
            || sample.boot_id != self.arm.clock.boot_id
            || now < before
            || now < *last
        {
            return Err(RelayOwnerError::Clock);
        }
        *last = now;
        let deadline = self
            .arm
            .deadline_ns_decimal
            .parse::<u128>()
            .map_err(|_| RelayOwnerError::Clock)?;
        let remaining = deadline
            .checked_sub(now)
            .filter(|left| *left > 0)
            .ok_or(RelayOwnerError::Expired)?;
        let nanos = u64::try_from(remaining).map_err(|_| RelayOwnerError::Clock)?;
        Ok(Duration::from_nanos(nanos))
    }
    pub(crate) async fn wait_expiration(&self) -> RelayOwnerError {
        loop {
            match self.remaining() {
                // A wakeup cadence is not an authority or a renewed timeout.
                // Each wake samples the same native boot/absolute Arm deadline.
                Ok(left) => tokio::time::sleep(left.min(Duration::from_millis(250))).await,
                Err(error) => return error,
            }
        }
    }
}
