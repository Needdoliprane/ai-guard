//! Coded control failures, without formatting an arbitrary error chain.
use crate::{
    broker::{
        enforcer::{EnforcerError, EnforcerStageFailure},
        lifecycle::LocalFailurePhaseV1,
    },
    runtime::RuntimePrestartStageFailure,
};
use serde::Serialize;
use std::fmt;

/// A fixed broker boundary or local error observation, never evidence that a
/// request was sent or ACKed.
/// A nested enforcer diagnostic can identify a more precise failing check.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BrokerStep {
    ActivitySpool,
    ArmExchange,
    Registration,
    Full3BindingPublication,
    ReadinessBeforeClaim,
    PreclaimChoice,
    ClockBeforeClaim,
    NativeClaimReserve,
    NativeClaimConsume,
    ClaimExchange,
    GenerationBeforePrepare,
    Prepare,
    OwnershipCommit,
    OwnershipReload,
    CreatedInspection,
    CreatedExchange,
    TerminalObservationAck,
    ResultAudit,
    CleanupConfirmation,
    CleanupAck,
    FailureObserved,
}
impl fmt::Display for BrokerStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("broker control stage failed")
    }
}
impl std::error::Error for BrokerStep {}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RegistrationStage {
    ReserveProspective,
    BeginExchange,
    BuildRegistration,
    BindProspective,
    RestoreAssigned,
    RegisterLegacy,
}

impl fmt::Display for RegistrationStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&BrokerStep::Registration, f)
    }
}

impl std::error::Error for RegistrationStage {}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RegistrationArmState {
    Pending,
    Acknowledged,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReservationMode {
    Fresh,
    Replay,
}

impl ReservationMode {
    pub(super) fn from_replay_only(replay_only: bool) -> Self {
        if replay_only {
            Self::Replay
        } else {
            Self::Fresh
        }
    }
}

impl fmt::Display for ReservationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&RegistrationStage::ReserveProspective, f)
    }
}

impl std::error::Error for ReservationMode {}

#[derive(Serialize)]
struct RegistrationDiagnostic {
    stage: RegistrationStage,
    arm_state: RegistrationArmState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_mode: Option<ReservationMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enforcer_code: Option<EnforcerError>,
}

impl RegistrationStage {
    fn diagnostic(self) -> RegistrationDiagnostic {
        // These stages are on either side of the validated Arm response at
        // the confirmed journal head. The state describes that local view.
        let arm_state = match self {
            Self::ReserveProspective | Self::BeginExchange => RegistrationArmState::Pending,
            Self::BuildRegistration
            | Self::BindProspective
            | Self::RestoreAssigned
            | Self::RegisterLegacy => RegistrationArmState::Acknowledged,
        };
        RegistrationDiagnostic {
            stage: self,
            arm_state,
            reservation_mode: None,
            enforcer_code: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum EffectEvidence {
    Unknown,
}

/// A local failure report is not a runtime terminal, fencing witness, absence
/// of a process, or permission to release a reservation.
#[derive(Serialize)]
pub(super) struct BatchFailureDiagnosticV1 {
    version: u32,
    phase: LocalFailurePhaseV1,
    runtime_effect: EffectEvidence,
    enforcer: Option<EnforcerStageFailure>,
    runtime: Option<RuntimePrestartStageFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    broker_step: Option<BrokerStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registration: Option<RegistrationDiagnostic>,
}

pub(super) fn classify(
    phase: LocalFailurePhaseV1,
    error: &anyhow::Error,
) -> Option<BatchFailureDiagnosticV1> {
    let enforcer = error.downcast_ref::<EnforcerStageFailure>().cloned();
    let runtime = error.downcast_ref::<RuntimePrestartStageFailure>().cloned();
    let registration = if let Some(mode) = error.downcast_ref::<ReservationMode>() {
        Some(RegistrationDiagnostic {
            reservation_mode: Some(*mode),
            // The code can originate locally or from the enforcer reply.
            enforcer_code: error.downcast_ref::<EnforcerError>().copied(),
            ..RegistrationStage::ReserveProspective.diagnostic()
        })
    } else {
        error
            .downcast_ref::<RegistrationStage>()
            .map(|stage| stage.diagnostic())
    };
    let broker_step = error
        .downcast_ref::<BrokerStep>()
        .copied()
        .or_else(|| registration.as_ref().map(|_| BrokerStep::Registration));
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
        registration,
    })
}

/// Report only the known boundary or the local observation of an error. The
/// fallback does not establish its cause, a durable lifecycle append, or any
/// runtime effect; specialized provenance remains unchanged when available.
pub(super) fn failure_report(
    phase: LocalFailurePhaseV1,
    error: &anyhow::Error,
) -> BatchFailureDiagnosticV1 {
    classify(phase, error).unwrap_or(BatchFailureDiagnosticV1 {
        version: 1,
        phase,
        runtime_effect: EffectEvidence::Unknown,
        enforcer: None,
        runtime: None,
        broker_step: Some(BrokerStep::FailureObserved),
        registration: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePrestartStage;
    use serde_json::json;

    #[test]
    fn registration_failure_reports_only_the_coded_boundary_and_arm_state() {
        for (stage, label, state) in [
            (
                RegistrationStage::ReserveProspective,
                "reserve_prospective",
                "pending",
            ),
            (
                RegistrationStage::BeginExchange,
                "begin_exchange",
                "pending",
            ),
            (
                RegistrationStage::BuildRegistration,
                "build_registration",
                "acknowledged",
            ),
            (
                RegistrationStage::BindProspective,
                "bind_prospective",
                "acknowledged",
            ),
            (
                RegistrationStage::RestoreAssigned,
                "restore_assigned",
                "acknowledged",
            ),
            (
                RegistrationStage::RegisterLegacy,
                "register_legacy",
                "acknowledged",
            ),
        ] {
            let error = anyhow::anyhow!("synthetic-private-detail").context(stage);
            let report =
                serde_json::to_value(failure_report(LocalFailurePhaseV1::FinalScan, &error))
                    .unwrap();
            assert_eq!(
                report,
                json!({
                    "version": 1,
                    "phase": "final_scan",
                    "runtime_effect": "unknown",
                    "enforcer": null,
                    "runtime": null,
                    "broker_step": "registration",
                    "registration": {"stage": label, "arm_state": state},
                })
            );
        }
    }

    #[test]
    fn prospective_reservation_reports_mode_and_typed_code_only() {
        for (replay_only, code, mode, label) in [
            (false, EnforcerError::Capacity, "fresh", "capacity"),
            (true, EnforcerError::Conflict, "replay", "conflict"),
        ] {
            let error = anyhow::Error::new(code)
                .context(ReservationMode::from_replay_only(replay_only))
                .context("synthetic private outer context");
            let report =
                serde_json::to_value(failure_report(LocalFailurePhaseV1::FinalScan, &error))
                    .unwrap();
            assert_eq!(
                report,
                json!({
                    "version": 1,
                    "phase": "final_scan",
                    "runtime_effect": "unknown",
                    "enforcer": null,
                    "runtime": null,
                    "broker_step": "registration",
                    "registration": {
                        "stage": "reserve_prospective",
                        "arm_state": "pending",
                        "reservation_mode": mode,
                        "enforcer_code": label,
                    },
                })
            );
        }
        let error = anyhow::anyhow!("synthetic private detail")
            .context(ReservationMode::from_replay_only(false));
        let report =
            serde_json::to_value(failure_report(LocalFailurePhaseV1::FinalScan, &error)).unwrap();
        assert_eq!(
            report["registration"],
            json!({"stage": "reserve_prospective", "arm_state": "pending",
                "reservation_mode": "fresh"})
        );
    }

    #[test]
    fn arbitrary_error_does_not_become_a_start_diagnostic() {
        let private = "private upstream text must not be persisted";
        let error = anyhow::anyhow!("{private}").context("private outer context");
        for phase in [
            LocalFailurePhaseV1::Decision,
            LocalFailurePhaseV1::WorkspacePreparation,
            LocalFailurePhaseV1::Admission,
            LocalFailurePhaseV1::Import,
            LocalFailurePhaseV1::ImportVerification,
            LocalFailurePhaseV1::Worker,
            LocalFailurePhaseV1::FinalScan,
            LocalFailurePhaseV1::Export,
            LocalFailurePhaseV1::Cleanup,
        ] {
            assert!(classify(phase, &error).is_none());
            let value = serde_json::to_value(failure_report(phase, &error)).unwrap();
            assert_eq!(
                value,
                json!({
                    "version": 1,
                    "phase": phase,
                    "runtime_effect": "unknown",
                    "enforcer": null,
                    "runtime": null,
                    "broker_step": "failure_observed",
                })
            );
            let bytes = serde_json::to_string(&value).unwrap();
            assert!(!bytes.contains(private));
            assert!(!bytes.contains("private outer context"));
        }
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
        assert_eq!(
            value,
            serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &error)).unwrap()
        );
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
    fn observe_only_reports_its_stage_without_claiming_a_runtime_effect() {
        let error = anyhow::Error::new(RuntimePrestartStageFailure::observe_only())
            .context("synthetic private reconciliation context");
        let diagnostic = failure_report(LocalFailurePhaseV1::ImportVerification, &error);
        assert_eq!(
            serde_json::to_value(diagnostic).unwrap(),
            json!({
                "version": 1,
                "phase": "import_verification",
                "runtime_effect": "unknown",
                "enforcer": null,
                "runtime": {"stage": "observe_only", "container_id": null},
            })
        );
    }

    #[test]
    fn broker_context_reports_only_the_fixed_boundary() {
        let private = "synthetic-private-journal-error-keep-out-of-audit";
        for (step, label) in [
            (BrokerStep::ActivitySpool, "activity_spool"),
            (BrokerStep::ArmExchange, "arm_exchange"),
            (BrokerStep::Registration, "registration"),
            (
                BrokerStep::Full3BindingPublication,
                "full3_binding_publication",
            ),
            (BrokerStep::ReadinessBeforeClaim, "readiness_before_claim"),
            (BrokerStep::PreclaimChoice, "preclaim_choice"),
            (BrokerStep::ClockBeforeClaim, "clock_before_claim"),
            (BrokerStep::NativeClaimReserve, "native_claim_reserve"),
            (BrokerStep::NativeClaimConsume, "native_claim_consume"),
            (BrokerStep::ClaimExchange, "claim_exchange"),
            (
                BrokerStep::GenerationBeforePrepare,
                "generation_before_prepare",
            ),
            (BrokerStep::Prepare, "prepare"),
            (BrokerStep::OwnershipCommit, "ownership_commit"),
            (BrokerStep::OwnershipReload, "ownership_reload"),
            (BrokerStep::CreatedInspection, "created_inspection"),
            (BrokerStep::CreatedExchange, "created_exchange"),
            (BrokerStep::FailureObserved, "failure_observed"),
        ] {
            let error = anyhow::anyhow!("{private}")
                .context(step)
                .context("private outer context");
            let diagnostic = classify(LocalFailurePhaseV1::Worker, &error).unwrap();
            let value = serde_json::to_value(diagnostic).unwrap();
            assert_eq!(
                value,
                json!({
                    "version": 1,
                    "phase": "worker",
                    "runtime_effect": "unknown",
                    "enforcer": null,
                    "runtime": null,
                    "broker_step": label,
                })
            );
            assert_eq!(
                value,
                serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &error)).unwrap()
            );
            let bytes = serde_json::to_string(&value).unwrap();
            assert!(!bytes.contains(private));
            assert!(!bytes.contains("private outer context"));
        }
    }
}
