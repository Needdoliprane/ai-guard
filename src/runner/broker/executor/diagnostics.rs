//! Coded control failures, without formatting an arbitrary error chain.
use crate::{
    broker::{enforcer::EnforcerStageFailure, lifecycle::LocalFailurePhaseV1},
    runtime::RuntimePrestartStageFailure,
};
use serde::Serialize;

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
}

pub(super) fn classify(
    phase: LocalFailurePhaseV1,
    error: &anyhow::Error,
) -> Option<BatchFailureDiagnosticV1> {
    let enforcer = error.downcast_ref::<EnforcerStageFailure>().cloned();
    let runtime = error.downcast_ref::<RuntimePrestartStageFailure>().cloned();
    if enforcer.is_none() && runtime.is_none() {
        return None;
    }
    Some(BatchFailureDiagnosticV1 {
        version: 1,
        phase,
        runtime_effect: EffectEvidence::Unknown,
        enforcer,
        runtime,
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
}
