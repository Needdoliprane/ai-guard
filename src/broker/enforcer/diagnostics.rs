//! Emitted local failure facts, never a wire request or a start capability.

use super::{EnforcerError, EnforcerSlotStatusV1, SlotPhaseV1};
use crate::control::{ArmV1, HostClockSampleV1};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EnforcerStage {
    Register,
    BeforeClaim,
    Prepare,
    BeforeSpawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EnforcerFailureReason {
    PermissionBindingMismatch,
    GenerationChanged,
    InvalidArm,
    HelperUnhealthy,
    ClockUnavailable,
    ArmClockIdentityMismatch,
    ArmClockRegressed,
    ArmClockRejected,
    DeadlineExpired,
    RegistrationFailed(EnforcerError),
    RemoteRejected(EnforcerError),
    PrepareOutcomeUnknown(EnforcerError),
    ResponseMismatch,
    ResponseBindingMismatch,
    TargetMismatch,
    SlotUnacknowledged,
    SlotAlreadyPrepared,
    SlotPhaseMismatch,
    SlotNotPrepared,
    SlotSnapshotStale,
    StopSignalled,
    LockPoisoned,
    ClockSnapshotMissing,
    NativeClockInvalid,
    SnapshotClockInvalid,
    SnapshotClockIdentityMismatch,
    ClockRegressed,
    ClockSnapshotStale,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SlotSnapshotSource {
    HelperSlot,
}

/// Retained helper facts, not a fresh local clock or a runtime observation.
#[derive(Clone, Serialize)]
struct SlotFailureSnapshot {
    source: SlotSnapshotSource,
    phase: SlotPhaseV1,
    prepared_once: bool,
    generation: u64,
    clock: Option<HostClockSampleV1>,
    clock_rejected: bool,
}

/// Local error provenance, deliberately not deserializable into authority.
/// A sample is the check's existing local native reading, not an error-time
/// timestamp. It is absent when that check had not reached its clock sample.
#[derive(Clone, Serialize)]
pub(crate) struct EnforcerStageFailure {
    version: u32,
    // Copied only from the canonical UUID of an already validated Arm.
    arm_id: String,
    stage: EnforcerStage,
    reason: EnforcerFailureReason,
    native_sample: Option<Box<HostClockSampleV1>>,
    original_deadline_ns_decimal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    slot_snapshot: Option<Box<SlotFailureSnapshot>>,
}
impl EnforcerStageFailure {
    /// Called only on a failed registration, with the original durable Arm.
    /// Validate before copying its bounded identifiers; no runtime-name,
    /// invocation or registration text enters the diagnostic. Registration did
    /// not expose a native check sample, so none is manufactured here.
    pub(crate) fn registration_failure(arm: &ArmV1, code: EnforcerError) -> Option<Self> {
        arm.validate().ok()?;
        let operation_id = uuid::Uuid::parse_str(&arm.operation_id).ok()?;
        if operation_id.is_nil() || operation_id.hyphenated().to_string() != arm.operation_id {
            return None;
        }
        Some(Self::from_arm(
            arm,
            EnforcerStage::Register,
            EnforcerFailureReason::RegistrationFailed(code),
            None,
        ))
    }

    pub(super) fn from_arm(
        arm: &ArmV1,
        stage: EnforcerStage,
        reason: EnforcerFailureReason,
        native_sample: Option<&HostClockSampleV1>,
    ) -> Self {
        Self {
            version: 1,
            arm_id: arm.operation_id.clone(),
            stage,
            reason,
            native_sample: native_sample.cloned().map(Box::new),
            original_deadline_ns_decimal: arm.deadline_ns_decimal.clone(),
            slot_snapshot: None,
        }
    }

    /// Called only after an existing slot predicate failed, while its status
    /// mutex is retained. Invalid helper clock text is never copied, and an
    /// absent clock remains unknown rather than a rejected or native reading.
    pub(super) fn with_slot_snapshot(mut self, status: &EnforcerSlotStatusV1) -> Self {
        let (clock, clock_rejected) = match status.clock.as_ref() {
            Some(clock) if clock.validate().is_ok() => (Some(clock.clone()), false),
            Some(_) => (None, true),
            None => (None, false),
        };
        self.slot_snapshot = Some(Box::new(SlotFailureSnapshot {
            source: SlotSnapshotSource::HelperSlot,
            phase: status.phase,
            prepared_once: status.prepared_once,
            generation: status.generation,
            clock,
            clock_rejected,
        }));
        self
    }

    /// Preserve the exact pre-diagnostic public/IPC error classification.
    pub(super) fn wire_code(&self) -> EnforcerError {
        use EnforcerFailureReason as Reason;
        match self.reason {
            Reason::PermissionBindingMismatch | Reason::ResponseBindingMismatch => {
                EnforcerError::Conflict
            }
            Reason::GenerationChanged if self.stage == EnforcerStage::Prepare => {
                EnforcerError::Conflict
            }
            Reason::InvalidArm => EnforcerError::InvalidInput,
            Reason::RegistrationFailed(code)
            | Reason::RemoteRejected(code)
            | Reason::PrepareOutcomeUnknown(code) => code,
            Reason::ResponseMismatch => EnforcerError::Unavailable,
            Reason::HelperUnhealthy
            | Reason::ClockUnavailable
            | Reason::LockPoisoned
            | Reason::ClockSnapshotMissing
            | Reason::NativeClockInvalid
            | Reason::SnapshotClockInvalid
            | Reason::SnapshotClockIdentityMismatch
            | Reason::ClockRegressed
            | Reason::ClockSnapshotStale => EnforcerError::Unhealthy,
            Reason::GenerationChanged
            | Reason::ArmClockIdentityMismatch
            | Reason::ArmClockRegressed
            | Reason::ArmClockRejected
            | Reason::DeadlineExpired
            | Reason::TargetMismatch
            | Reason::SlotUnacknowledged
            | Reason::SlotAlreadyPrepared
            | Reason::SlotPhaseMismatch
            | Reason::SlotNotPrepared
            | Reason::SlotSnapshotStale
            | Reason::StopSignalled => EnforcerError::NotReady,
        }
    }
}
impl fmt::Debug for EnforcerStageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EnforcerStageFailure(<private>)")
    }
}
impl fmt::Display for EnforcerStageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("private enforcer start check failed")
    }
}
impl std::error::Error for EnforcerStageFailure {}

/// Called only after the existing Arm clock validator rejected the sample.
/// Classification adds no sample, RPC, hash or validation on the success path.
pub(super) fn arm_clock_rejection(
    original: &HostClockSampleV1,
    deadline: &str,
    sample: &HostClockSampleV1,
) -> EnforcerFailureReason {
    if original.method != sample.method || original.boot_id != sample.boot_id {
        return EnforcerFailureReason::ArmClockIdentityMismatch;
    }
    match (
        sample.continuous_ns(),
        original.continuous_ns(),
        deadline.parse::<u128>(),
    ) {
        (Ok(current), Ok(armed), _) if current < armed => EnforcerFailureReason::ArmClockRegressed,
        (Ok(current), _, Ok(deadline)) if current >= deadline => {
            EnforcerFailureReason::DeadlineExpired
        }
        _ => EnforcerFailureReason::ArmClockRejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        ArmSubjectV1, HostClockMethodV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
        RuntimeKindV1, SecurityPresetV1,
    };

    fn sample(ns: u128) -> HostClockSampleV1 {
        HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::LinuxBoottimeV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: ns.to_string(),
            observed_utc: None,
        }
    }

    fn failure(reason: EnforcerFailureReason) -> EnforcerStageFailure {
        EnforcerStageFailure {
            version: 1,
            arm_id: "22222222-2222-4222-8222-222222222222".into(),
            stage: EnforcerStage::Prepare,
            reason,
            native_sample: Some(Box::new(sample(5))),
            original_deadline_ns_decimal: "6".into(),
            slot_snapshot: None,
        }
    }

    fn arm() -> ArmV1 {
        ArmV1 {
            version: 1,
            operation_id: "22222222-2222-4222-8222-222222222222".into(),
            worker_id: "33333333-3333-4333-8333-333333333333".into(),
            attempt_id: "44444444-4444-4444-8444-444444444444".into(),
            expected_revision: 1,
            intent_hash: "a".repeat(64),
            subject: ArmSubjectV1::Worker,
            inspection: RuntimeInspectionV1 {
                authority_id: "55555555-5555-4555-8555-555555555555".into(),
                runtime_kind: RuntimeKindV1::Docker,
                container_id: "b".repeat(64),
                stable_name: "private-runtime-name-canary".into(),
                volume_name: "private-volume-name-canary".into(),
                image_id: format!("sha256:{}", "c".repeat(64)),
                spec_hash: "d".repeat(64),
                cpu_millis: 1000,
                memory_bytes: 1024 * 1024,
                pids_limit: 32,
                network: NetworkV1::None,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
            },
            authority_binding_hash: "e".repeat(64),
            configuration_fingerprint_version: 2,
            configuration_fingerprint: "f".repeat(64),
            clock: sample(10),
            timeout_seconds: 1,
            deadline_ns_decimal: "1000000010".into(),
        }
    }

    fn slot(clock: Option<HostClockSampleV1>) -> EnforcerSlotStatusV1 {
        EnforcerSlotStatusV1 {
            arm_id: "private-helper-arm-canary".into(),
            registration_hash: "private-registration-hash-canary".into(),
            phase: SlotPhaseV1::Quarantined,
            prepared_once: false,
            generation: 7,
            clock,
            observation: None,
            stop_failures: 0,
        }
    }

    #[test]
    fn helper_slot_snapshot_keeps_validated_helper_clock_distinct_from_native_sample() {
        let status = slot(Some(sample(17)));
        for stage in [EnforcerStage::BeforeClaim, EnforcerStage::BeforeSpawn] {
            let diagnostic = EnforcerStageFailure::from_arm(
                &arm(),
                stage,
                EnforcerFailureReason::SlotPhaseMismatch,
                None,
            )
            .with_slot_snapshot(&status);
            assert_eq!(diagnostic.wire_code(), EnforcerError::NotReady);
            let value = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(value["reason"], "slot_phase_mismatch");
            assert!(value["native_sample"].is_null());
            assert_eq!(
                value["slot_snapshot"],
                serde_json::json!({
                    "source": "helper_slot",
                    "phase": "quarantined",
                    "prepared_once": false,
                    "generation": 7,
                    "clock": sample(17),
                    "clock_rejected": false,
                })
            );
            assert_eq!(value.as_object().unwrap().len(), 7);
            let serialized = serde_json::to_string(&diagnostic).unwrap();
            assert!(!serialized.contains("private-helper-arm-canary"));
            assert!(!serialized.contains("private-registration-hash-canary"));
        }
    }

    #[test]
    fn helper_slot_snapshot_rejects_invalid_clock_without_copying_private_text() {
        let valid = sample(17);
        let mut invalid_clocks = Vec::new();
        for boot_id in [
            "private-helper-clock-canary",
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
            "00000000-0000-0000-0000-000000000000",
        ] {
            let mut invalid = valid.clone();
            invalid.boot_id = boot_id.into();
            invalid_clocks.push(invalid);
        }
        for decimal in [
            "private-helper-clock-canary".into(),
            "017".into(),
            "1".repeat(40),
        ] {
            let mut invalid = valid.clone();
            invalid.continuous_ns_decimal = decimal;
            invalid_clocks.push(invalid);
        }
        let mut invalid = valid.clone();
        invalid.observed_utc = Some("private-helper-clock-canary".into());
        invalid_clocks.push(invalid);
        let mut invalid = valid;
        invalid.version = 2;
        invalid_clocks.push(invalid);
        for invalid in invalid_clocks {
            let diagnostic = EnforcerStageFailure::from_arm(
                &arm(),
                EnforcerStage::BeforeClaim,
                EnforcerFailureReason::SlotPhaseMismatch,
                None,
            )
            .with_slot_snapshot(&slot(Some(invalid)));
            let value = serde_json::to_value(&diagnostic).unwrap();
            assert!(value["native_sample"].is_null());
            assert!(value["slot_snapshot"]["clock"].is_null());
            assert_eq!(value["slot_snapshot"]["clock_rejected"], true);
            assert_eq!(value["slot_snapshot"]["phase"], "quarantined");
            assert_eq!(diagnostic.wire_code(), EnforcerError::NotReady);
            assert!(
                !serde_json::to_string(&diagnostic)
                    .unwrap()
                    .contains("private-helper-clock-canary")
            );
        }
    }

    #[test]
    fn missing_helper_clock_stays_unknown_and_legacy_diagnostics_keep_six_fields() {
        let diagnostic = EnforcerStageFailure::from_arm(
            &arm(),
            EnforcerStage::BeforeClaim,
            EnforcerFailureReason::SlotPhaseMismatch,
            None,
        );
        let legacy = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(legacy.as_object().unwrap().len(), 6);
        assert!(legacy.get("slot_snapshot").is_none());
        let retained = diagnostic.clone().with_slot_snapshot(&slot(None));
        let value = serde_json::to_value(&retained).unwrap();
        assert!(value["slot_snapshot"]["clock"].is_null());
        assert_eq!(value["slot_snapshot"]["clock_rejected"], false);
        assert!(value["native_sample"].is_null());
        assert_eq!(retained.wire_code(), diagnostic.wire_code());
        assert_eq!(serde_json::to_value(&diagnostic).unwrap(), legacy);
    }

    #[test]
    fn registration_failure_keeps_only_validated_arm_metadata_and_closed_error_code() {
        let arm = arm();
        for code in [
            EnforcerError::InvalidInput,
            EnforcerError::UnsafeFilesystem,
            EnforcerError::Unauthorized,
            EnforcerError::Unavailable,
            EnforcerError::Conflict,
            EnforcerError::Capacity,
            EnforcerError::NotReady,
            EnforcerError::AlreadyPrepared,
            EnforcerError::Unhealthy,
            EnforcerError::NotIdle,
        ] {
            let diagnostic = EnforcerStageFailure::registration_failure(&arm, code).unwrap();
            assert_eq!(diagnostic.wire_code(), code);
            let value = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(value["stage"], "register");
            assert_eq!(value["arm_id"], arm.operation_id);
            assert_eq!(
                value["original_deadline_ns_decimal"],
                arm.deadline_ns_decimal
            );
            assert!(value["native_sample"].is_null());
            assert_eq!(
                value["reason"]["registration_failed"],
                serde_json::to_value(code).unwrap()
            );
            assert_eq!(value.as_object().unwrap().len(), 6);
            let serialized = serde_json::to_string(&diagnostic).unwrap();
            assert!(!serialized.contains("private-runtime-name-canary"));
            assert!(!serialized.contains("private-volume-name-canary"));
            for text in [diagnostic.to_string(), format!("{diagnostic:?}")] {
                assert!(!text.contains(&arm.operation_id));
                assert!(!text.contains(&arm.deadline_ns_decimal));
                assert!(!text.contains("registration_failed"));
            }
        }
    }

    #[test]
    fn registration_failure_rejects_noncanonical_or_invalid_arm_metadata() {
        let valid = arm();
        for operation_id in [
            "private-operation-canary".into(),
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".into(),
            "00000000-0000-0000-0000-000000000000".into(),
            "a".repeat(4096),
        ] {
            let mut invalid = valid.clone();
            invalid.operation_id = operation_id;
            assert!(
                EnforcerStageFailure::registration_failure(&invalid, EnforcerError::InvalidInput)
                    .is_none()
            );
        }
        for deadline in [
            "private-deadline-canary".into(),
            "01000000010".into(),
            "1000000011".into(),
            "1".repeat(40),
        ] {
            let mut invalid = valid.clone();
            invalid.deadline_ns_decimal = deadline;
            assert!(
                EnforcerStageFailure::registration_failure(&invalid, EnforcerError::InvalidInput)
                    .is_none()
            );
        }
        let mut invalid = valid;
        invalid.version = 2;
        assert!(
            EnforcerStageFailure::registration_failure(&invalid, EnforcerError::InvalidInput)
                .is_none()
        );
    }

    #[test]
    fn preclaim_failure_reasons_preserve_public_codes_without_inventing_a_sample() {
        for (reason, code) in [
            (
                EnforcerFailureReason::SlotUnacknowledged,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::SlotAlreadyPrepared,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::SlotPhaseMismatch,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::GenerationChanged,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::SlotSnapshotStale,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::StopSignalled,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::HelperUnhealthy,
                EnforcerError::Unhealthy,
            ),
            (
                EnforcerFailureReason::LockPoisoned,
                EnforcerError::Unhealthy,
            ),
            (
                EnforcerFailureReason::ClockUnavailable,
                EnforcerError::Unhealthy,
            ),
            (
                EnforcerFailureReason::DeadlineExpired,
                EnforcerError::NotReady,
            ),
        ] {
            let diagnostic =
                EnforcerStageFailure::from_arm(&arm(), EnforcerStage::BeforeClaim, reason, None);
            assert_eq!(diagnostic.wire_code(), code);
            let value = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(value["stage"], "before_claim");
            assert!(value["native_sample"].is_null());
        }
        let check = sample(1_000_000_010);
        let arm = arm();
        let diagnostic = EnforcerStageFailure::from_arm(
            &arm,
            EnforcerStage::BeforeClaim,
            arm_clock_rejection(&arm.clock, &arm.deadline_ns_decimal, &check),
            Some(&check),
        );
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["reason"], "deadline_expired");
        assert_eq!(
            value["native_sample"]["continuous_ns_decimal"],
            "1000000010"
        );
    }

    #[test]
    fn opaque_diagnostic_survives_anyhow_context_without_free_form_data() {
        let diagnostic = failure(EnforcerFailureReason::DeadlineExpired);
        let expected = serde_json::to_value(&diagnostic).unwrap();
        let error = anyhow::Error::new(diagnostic.clone()).context("outer-test-context-canary");
        let retained = error.downcast_ref::<EnforcerStageFailure>().unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), expected);
        assert_eq!(expected["version"], 1);
        assert_eq!(expected["native_sample"]["continuous_ns_decimal"], "5");
        assert_eq!(expected["original_deadline_ns_decimal"], "6");
        assert!(expected["native_sample"]["observed_utc"].is_null());
        assert_eq!(expected.as_object().unwrap().len(), 6);
        for text in [diagnostic.to_string(), format!("{diagnostic:?}")] {
            assert!(!text.contains("22222222"));
            assert!(!text.contains("deadline"));
            assert!(!text.contains("outer-test-context-canary"));
        }
    }

    #[test]
    fn unknown_prepare_outcome_is_distinct_from_rejection_and_keeps_wire_codes() {
        let unknown = failure(EnforcerFailureReason::PrepareOutcomeUnknown(
            EnforcerError::Unavailable,
        ));
        let rejected = failure(EnforcerFailureReason::RemoteRejected(
            EnforcerError::Unavailable,
        ));
        assert_eq!(unknown.wire_code(), EnforcerError::Unavailable);
        assert_eq!(rejected.wire_code(), EnforcerError::Unavailable);
        assert_ne!(
            serde_json::to_value(&unknown).unwrap()["reason"],
            serde_json::to_value(&rejected).unwrap()["reason"]
        );
        assert_eq!(
            serde_json::to_string(&unknown.wire_code()).unwrap(),
            "\"unavailable\""
        );
        for (reason, prepare, guard) in [
            (
                EnforcerFailureReason::GenerationChanged,
                EnforcerError::Conflict,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::DeadlineExpired,
                EnforcerError::NotReady,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::SlotNotPrepared,
                EnforcerError::NotReady,
                EnforcerError::NotReady,
            ),
            (
                EnforcerFailureReason::ClockSnapshotStale,
                EnforcerError::Unhealthy,
                EnforcerError::Unhealthy,
            ),
        ] {
            let mut diagnostic = failure(reason);
            assert_eq!(diagnostic.wire_code(), prepare);
            diagnostic.stage = EnforcerStage::BeforeSpawn;
            assert_eq!(diagnostic.wire_code(), guard);
        }
    }

    #[test]
    fn rejected_arm_clock_classifies_deadline_identity_and_regression() {
        let original = sample(10);
        assert_eq!(
            arm_clock_rejection(&original, "20", &sample(20)),
            EnforcerFailureReason::DeadlineExpired
        );
        assert_eq!(
            arm_clock_rejection(&original, "20", &sample(9)),
            EnforcerFailureReason::ArmClockRegressed
        );
        let mut other_boot = sample(11);
        other_boot.boot_id = "33333333-3333-4333-8333-333333333333".into();
        assert_eq!(
            arm_clock_rejection(&original, "20", &other_boot),
            EnforcerFailureReason::ArmClockIdentityMismatch
        );
        assert_eq!(
            arm_clock_rejection(&original, "invalid", &sample(11)),
            EnforcerFailureReason::ArmClockRejected
        );
    }
}
