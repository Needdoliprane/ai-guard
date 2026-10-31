//! Coded control failures, without formatting an arbitrary error chain.
use crate::{
    broker::{enforcer::EnforcerStageFailure, lifecycle::LocalFailurePhaseV1},
    runtime::RuntimePrestartStageFailure,
};
use serde::Serialize;
use std::fmt;

/// An outer control boundary, never evidence that a request was sent or ACKed.
/// A nested enforcer diagnostic can identify a more precise failing check.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PreclaimStep {
    ArmExchange,
    Registration,
    LoadAfterRegistration,
    ReadinessBeforeClaim,
    ClockBeforeClaim,
    ClaimExchange,
    GenerationBeforePrepare,
    Prepare,
}
impl fmt::Display for PreclaimStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("broker preclaim stage failed")
    }
}
impl std::error::Error for PreclaimStep {}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum EffectEvidence {
    Unknown,
}

/// An error at a trusted control boundary is not a runtime terminal, fencing
/// witness, absence of a process, or permission to release a reservation.
#[derive(Serialize)]
pub(super) struct BatchFailureDiagnosticV1 {
    version: u32,
    phase: LocalFailurePhaseV1,
    runtime_effect: EffectEvidence,
    enforcer: Option<EnforcerStageFailure>,
    runtime: Option<RuntimePrestartStageFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    broker_step: Option<PreclaimStep>,
}

pub(super) fn classify(
    phase: LocalFailurePhaseV1,
    error: &anyhow::Error,
) -> Option<BatchFailureDiagnosticV1> {
    let enforcer = error.downcast_ref::<EnforcerStageFailure>().cloned();
    let runtime = error.downcast_ref::<RuntimePrestartStageFailure>().cloned();
    let broker_step = error.downcast_ref::<PreclaimStep>().copied();
    if enforcer.is_none() && runtime.is_none() && broker_step.is_none() {
        return None;
    }
    Some(BatchFailureDiagnosticV1 {
        version: 1,
        phase,
        runtime_effect: EffectEvidence::Unknown,
        enforcer,
        runtime,
        broker_step,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePrestartStage;
    use serde_json::json;

    #[test]
    fn arbitrary_error_does_not_become_a_start_diagnostic() {
        let error = anyhow::anyhow!("private upstream text must not be persisted");
        assert!(classify(LocalFailurePhaseV1::Worker, &error).is_none());
    }

    #[test]
    fn coded_stage_survives_context_without_serializing_private_error_text() {
        let private = "synthetic-private-upstream-value-keep-out-of-audit";
        let cid = "a".repeat(64);
        let error = anyhow::anyhow!("{private}")
            .context(RuntimePrestartStageFailure::new(
                RuntimePrestartStage::GuardBeforeSpawn,
                &cid,
            ))
            .context("another private transport context");
        let diagnostic = classify(LocalFailurePhaseV1::Worker, &error).unwrap();
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["version"], json!(1));
        assert_eq!(value["runtime_effect"], "unknown");
        assert_eq!(value["runtime"]["stage"], "guard_before_spawn");
        assert_eq!(value["runtime"]["container_id"], cid);
        assert!(value["enforcer"].is_null());
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains(private));
        assert!(!serialized.contains("another private transport context"));
    }

    #[test]
    fn preclaim_context_reports_only_the_fixed_boundary() {
        let private = "synthetic-private-journal-error-keep-out-of-audit";
        let error = anyhow::anyhow!("{private}")
            .context(PreclaimStep::LoadAfterRegistration)
            .context("private outer context");
        let diagnostic = classify(LocalFailurePhaseV1::Worker, &error).unwrap();
        let value = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(value["broker_step"], "load_after_registration");
        assert_eq!(value["runtime_effect"], "unknown");
        assert!(value["enforcer"].is_null());
        assert!(value["runtime"].is_null());
        let bytes = serde_json::to_string(&value).unwrap();
        assert!(!bytes.contains(private));
        assert!(!bytes.contains("private outer context"));
    }
}
