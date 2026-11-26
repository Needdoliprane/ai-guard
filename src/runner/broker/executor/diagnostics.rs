//! Coded control failures, without formatting an arbitrary error chain.
use crate::{
    broker::{
        enforcer::{
            EnforcerError, EnforcerStageFailure, LegacyReservationMotifV1,
            LegacyReservationRefusalV1,
        },
        journal::JournalError,
        lifecycle::LocalFailurePhaseV1,
        writer::WriterCallFailure,
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
    CreatedExchangeReload,
    CreatedExchangePreparePending,
    CreatedExchangeAdvancePending,
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

#[derive(Clone, Copy, Debug)]
pub(super) enum CreatedExchangeStage {
    Reload(CreatedReloadSite),
    PreparePending,
    AdvancePending,
    AdvanceAck,
}

impl CreatedExchangeStage {
    fn step(self) -> BrokerStep {
        match self {
            Self::Reload(_) => BrokerStep::CreatedExchangeReload,
            Self::PreparePending => BrokerStep::CreatedExchangePreparePending,
            Self::AdvancePending => BrokerStep::CreatedExchangeAdvancePending,
            // The optional diagnostic distinguishes the ACK write while
            // preserving the existing outer code for readers of V1.
            Self::AdvanceAck => BrokerStep::CreatedExchange,
        }
    }

    fn advance_stage(self) -> Option<CreatedAdvanceStage> {
        match self {
            Self::AdvancePending => Some(CreatedAdvanceStage::Pending),
            Self::AdvanceAck => Some(CreatedAdvanceStage::Acknowledgement),
            _ => None,
        }
    }
}

impl fmt::Display for CreatedExchangeStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&BrokerStep::CreatedExchange, f)
    }
}
impl std::error::Error for CreatedExchangeStage {}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CreatedReloadSite {
    MutationComposition,
    RpcLookup,
    CasReload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum WriterFailureCause {
    QueueFull,
    ReplyTimeout,
    Unavailable,
    ActorError,
}

#[derive(Debug, Serialize)]
pub(super) struct CreatedReloadDiagnostic {
    site: CreatedReloadSite,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<WriterFailureCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_code: Option<&'static str>,
}

impl CreatedReloadSite {
    fn diagnostic(self, error: &anyhow::Error) -> CreatedReloadDiagnostic {
        let (cause, journal_code) = writer_failure(error);
        CreatedReloadDiagnostic {
            site: self,
            cause,
            journal_code,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CreatedAdvanceStage {
    Pending,
    Acknowledgement,
}

#[derive(Debug, Serialize)]
pub(super) struct CreatedAdvanceDiagnostic {
    stage: CreatedAdvanceStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<WriterFailureCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_code: Option<&'static str>,
}

fn writer_failure(error: &anyhow::Error) -> (Option<WriterFailureCause>, Option<&'static str>) {
    use WriterFailureCause as Cause;
    match error.downcast_ref::<WriterCallFailure>() {
        Some(WriterCallFailure::QueueFull) => (Some(Cause::QueueFull), None),
        Some(WriterCallFailure::ReplyTimeout) => (Some(Cause::ReplyTimeout), None),
        Some(WriterCallFailure::Unavailable) => (Some(Cause::Unavailable), None),
        Some(WriterCallFailure::Actor(error)) => {
            (Some(Cause::ActorError), Some(journal_failure_code(error)))
        }
        None => (None, None),
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OwnershipCommitStage {
    Reload,
    Snapshot,
    Checkpoint,
    Advance,
    Receipt,
    ConflictLimit,
}

impl fmt::Display for OwnershipCommitStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&BrokerStep::OwnershipCommit, f)
    }
}
impl std::error::Error for OwnershipCommitStage {}

#[derive(Debug, Serialize)]
pub(super) struct OwnershipCommitDiagnostic {
    stage: OwnershipCommitStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<WriterFailureCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_code: Option<&'static str>,
}

impl OwnershipCommitStage {
    fn diagnostic(self, error: &anyhow::Error) -> OwnershipCommitDiagnostic {
        let (cause, journal_code) = match self {
            Self::Reload | Self::Advance => writer_failure(error),
            _ => (None, None),
        };
        OwnershipCommitDiagnostic {
            stage: self,
            cause,
            journal_code,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CreateIssuedStage {
    // The client did not return a typed result. The gateway's outcome is unknown.
    HttpExchange,
    // The client returned a typed result. These stages concern only its local ACK.
    AckReload,
    AckPrepare,
    AckAdvance,
    AckCasLimit,
}

impl fmt::Display for CreateIssuedStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("broker CreateIssued exchange failed")
    }
}

impl std::error::Error for CreateIssuedStage {}

#[derive(Debug, Serialize)]
pub(super) struct CreateIssuedDiagnostic {
    stage: CreateIssuedStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<WriterFailureCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_code: Option<&'static str>,
}

impl CreateIssuedStage {
    fn diagnostic(self, error: &anyhow::Error) -> CreateIssuedDiagnostic {
        let (cause, journal_code) = match self {
            Self::AckReload | Self::AckAdvance => writer_failure(error),
            _ => (None, None),
        };
        CreateIssuedDiagnostic {
            stage: self,
            cause,
            journal_code,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Full3BindingStage {
    ReadNativeBinding,
    ReloadJournal,
    AdvanceWriter,
    RetryLimit,
}

impl fmt::Display for Full3BindingStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&BrokerStep::Full3BindingPublication, f)
    }
}

impl std::error::Error for Full3BindingStage {}

#[derive(Debug, Serialize)]
pub(super) struct Full3BindingDiagnostic {
    stage: Full3BindingStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    enforcer_code: Option<EnforcerError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    journal_code: Option<&'static str>,
}

impl Full3BindingStage {
    fn diagnostic(self, error: &anyhow::Error) -> Full3BindingDiagnostic {
        let enforcer_code = match self {
            Self::ReadNativeBinding => error.downcast_ref::<EnforcerError>().copied(),
            _ => None,
        };
        let journal_code = match self {
            Self::ReloadJournal | Self::AdvanceWriter => error
                .downcast_ref::<JournalError>()
                .map(journal_failure_code),
            Self::RetryLimit => Some(journal_failure_code(&JournalError::Conflict)),
            Self::ReadNativeBinding => None,
        };
        Full3BindingDiagnostic {
            stage: self,
            enforcer_code,
            journal_code,
        }
    }
}

fn journal_failure_code(error: &JournalError) -> &'static str {
    match error {
        JournalError::InvalidInput => "invalid_input",
        JournalError::UnsupportedVersion => "unsupported_version",
        JournalError::TooLarge => "too_large",
        JournalError::LimitExceeded => "limit_exceeded",
        JournalError::Busy => "busy",
        JournalError::Conflict => "conflict",
        JournalError::Interrupted => "interrupted",
        JournalError::Corrupt => "corrupt",
        JournalError::UnsafeFilesystem => "unsafe_filesystem",
        JournalError::ControlSecret => "control_secret",
        JournalError::Io => "io",
    }
}

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

/// Carries the closed Legacy refusal of one reservation attempt into the
/// diagnostic chain. The motif is absent when the peer did not negotiate
/// support or the response was lost; it is never guessed.
#[derive(Clone, Copy, Debug)]
pub(super) struct ReservationRefusal(pub(super) LegacyReservationRefusalV1);

impl fmt::Display for ReservationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&RegistrationStage::ReserveProspective, f)
    }
}

impl std::error::Error for ReservationRefusal {}

#[derive(Serialize)]
struct RegistrationDiagnostic {
    stage: RegistrationStage,
    arm_state: RegistrationArmState,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_mode: Option<ReservationMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enforcer_code: Option<EnforcerError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_motif: Option<LegacyReservationMotifV1>,
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
            reservation_motif: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    full3_binding: Option<Full3BindingDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_reload: Option<CreatedReloadDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_advance: Option<CreatedAdvanceDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ownership_commit: Option<OwnershipCommitDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    create_issued: Option<CreateIssuedDiagnostic>,
}

impl BatchFailureDiagnosticV1 {
    pub(super) fn full3_binding(&self) -> Option<&Full3BindingDiagnostic> {
        self.full3_binding.as_ref()
    }

    pub(super) fn created_reload(&self) -> Option<&CreatedReloadDiagnostic> {
        self.created_reload.as_ref()
    }

    pub(super) fn created_advance(&self) -> Option<&CreatedAdvanceDiagnostic> {
        self.created_advance.as_ref()
    }

    pub(super) fn ownership_commit(&self) -> Option<&OwnershipCommitDiagnostic> {
        self.ownership_commit.as_ref()
    }

    pub(super) fn create_issued(&self) -> Option<&CreateIssuedDiagnostic> {
        self.create_issued.as_ref()
    }

    pub(super) fn created_exchange_step(&self) -> Option<BrokerStep> {
        self.broker_step.filter(|step| {
            matches!(
                step,
                BrokerStep::CreatedExchangeReload
                    | BrokerStep::CreatedExchangePreparePending
                    | BrokerStep::CreatedExchangeAdvancePending
            )
        })
    }
}

pub(super) fn classify(
    phase: LocalFailurePhaseV1,
    error: &anyhow::Error,
) -> Option<BatchFailureDiagnosticV1> {
    let enforcer = error.downcast_ref::<EnforcerStageFailure>().cloned();
    let runtime = error.downcast_ref::<RuntimePrestartStageFailure>().cloned();
    let registration = if let Some(mode) = error.downcast_ref::<ReservationMode>() {
        let refusal = error.downcast_ref::<ReservationRefusal>().map(|r| r.0);
        Some(RegistrationDiagnostic {
            reservation_mode: Some(*mode),
            // The code can originate locally or from the enforcer reply.
            enforcer_code: refusal
                .map(|r| r.code())
                .or_else(|| error.downcast_ref::<EnforcerError>().copied()),
            reservation_motif: refusal.and_then(|r| r.motif),
            ..RegistrationStage::ReserveProspective.diagnostic()
        })
    } else {
        error
            .downcast_ref::<RegistrationStage>()
            .map(|stage| stage.diagnostic())
    };
    let full3_stage = error.downcast_ref::<Full3BindingStage>().copied();
    let created_stage = error.downcast_ref::<CreatedExchangeStage>().copied();
    let create_issued = error
        .downcast_ref::<CreateIssuedStage>()
        .map(|stage| stage.diagnostic(error));
    let advance_stage = created_stage.and_then(CreatedExchangeStage::advance_stage);
    let broker_step = match error.downcast_ref::<BrokerStep>().copied() {
        Some(BrokerStep::CreatedExchange) => {
            Some(created_stage.map_or(BrokerStep::CreatedExchange, |stage| stage.step()))
        }
        step => step
            .or_else(|| full3_stage.map(|_| BrokerStep::Full3BindingPublication))
            .or_else(|| registration.as_ref().map(|_| BrokerStep::Registration))
            .or_else(|| {
                advance_stage
                    .and(created_stage)
                    .map(CreatedExchangeStage::step)
            })
            // Existing V1 readers know this outer label.
            .or_else(|| create_issued.as_ref().map(|_| BrokerStep::FailureObserved)),
    };
    let full3_binding = match broker_step {
        Some(BrokerStep::Full3BindingPublication) => {
            full3_stage.map(|stage| stage.diagnostic(error))
        }
        _ => None,
    };
    let created_reload = match (broker_step, created_stage) {
        (Some(BrokerStep::CreatedExchangeReload), Some(CreatedExchangeStage::Reload(site))) => {
            Some(site.diagnostic(error))
        }
        _ => None,
    };
    let created_advance = match (broker_step, advance_stage) {
        (
            Some(BrokerStep::CreatedExchangeAdvancePending),
            Some(stage @ CreatedAdvanceStage::Pending),
        )
        | (Some(BrokerStep::CreatedExchange), Some(stage @ CreatedAdvanceStage::Acknowledgement)) =>
        {
            let (cause, journal_code) = writer_failure(error);
            Some(CreatedAdvanceDiagnostic {
                stage,
                cause,
                journal_code,
            })
        }
        _ => None,
    };
    let ownership_commit = match broker_step {
        Some(BrokerStep::OwnershipCommit) => error
            .downcast_ref::<OwnershipCommitStage>()
            .map(|stage| stage.diagnostic(error)),
        _ => None,
    };
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
        full3_binding,
        created_reload,
        created_advance,
        ownership_commit,
        create_issued,
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
        full3_binding: None,
        created_reload: None,
        created_advance: None,
        ownership_commit: None,
        create_issued: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePrestartStage;
    use serde_json::json;

    #[test]
    fn ownership_commit_preserves_v1_and_reports_only_closed_details() {
        let old = json!({
            "version":1, "phase":"worker", "runtime_effect":"unknown",
            "enforcer":null, "runtime":null, "broker_step":"ownership_commit",
        });
        let private = "synthetic-private-ownership-detail";
        let previous = anyhow::anyhow!("{private}").context(BrokerStep::OwnershipCommit);
        assert_eq!(
            serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &previous)).unwrap(),
            old
        );
        for (stage, cause, expected) in [
            (
                OwnershipCommitStage::Reload,
                Some(WriterCallFailure::QueueFull),
                json!({"stage":"reload","cause":"queue_full"}),
            ),
            (
                OwnershipCommitStage::Snapshot,
                None,
                json!({"stage":"snapshot"}),
            ),
            (
                OwnershipCommitStage::Checkpoint,
                None,
                json!({"stage":"checkpoint"}),
            ),
            (
                OwnershipCommitStage::Advance,
                Some(WriterCallFailure::Actor(JournalError::Io)),
                json!({"stage":"advance","cause":"actor_error","journal_code":"io"}),
            ),
            (
                OwnershipCommitStage::Receipt,
                None,
                json!({"stage":"receipt"}),
            ),
            (
                OwnershipCommitStage::ConflictLimit,
                None,
                json!({"stage":"conflict_limit"}),
            ),
        ] {
            let error = anyhow::anyhow!("{private}");
            let error = match cause {
                Some(cause) => error.context(cause),
                None => error,
            };
            let error = error.context(stage).context(BrokerStep::OwnershipCommit);
            let diagnostic = failure_report(LocalFailurePhaseV1::Worker, &error);
            let mut wanted = old.clone();
            wanted["ownership_commit"] = expected;
            assert_eq!(serde_json::to_value(&diagnostic).unwrap(), wanted);
            assert!(!format!("{:?}", diagnostic.ownership_commit()).contains(private));
        }
        let other = anyhow::anyhow!("{private}")
            .context(OwnershipCommitStage::Reload)
            .context(BrokerStep::OwnershipReload);
        let report =
            serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &other)).unwrap();
        assert_eq!(report["broker_step"], "ownership_reload");
        assert!(report.get("ownership_commit").is_none());
    }

    #[test]
    fn create_issued_reports_http_and_local_ack_as_distinct_boundaries() {
        let private = "synthetic-private-create-issued-detail";
        for (stage, cause, expected) in [
            (
                CreateIssuedStage::HttpExchange,
                None,
                json!({"stage":"http_exchange"}),
            ),
            (
                CreateIssuedStage::AckReload,
                Some(WriterCallFailure::Actor(JournalError::Corrupt)),
                json!({"stage":"ack_reload","cause":"actor_error","journal_code":"corrupt"}),
            ),
            (
                CreateIssuedStage::AckPrepare,
                None,
                json!({"stage":"ack_prepare"}),
            ),
            (
                CreateIssuedStage::AckAdvance,
                Some(WriterCallFailure::ReplyTimeout),
                json!({"stage":"ack_advance","cause":"reply_timeout"}),
            ),
            (
                CreateIssuedStage::AckCasLimit,
                None,
                json!({"stage":"ack_cas_limit"}),
            ),
        ] {
            let error = anyhow::anyhow!("{private}");
            let error = match cause {
                Some(cause) => error.context(cause),
                None => error,
            };
            let diagnostic = failure_report(LocalFailurePhaseV1::Worker, &error.context(stage));
            let report = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(
                report,
                json!({
                    "version":1, "phase":"worker", "runtime_effect":"unknown",
                    "enforcer":null, "runtime":null, "broker_step":"failure_observed",
                    "create_issued":expected,
                })
            );
            assert!(!format!("{:?}", diagnostic.create_issued()).contains(private));
        }
    }

    #[test]
    fn created_exchange_reports_the_local_stage_without_claiming_publication() {
        let private = "synthetic-private-created-error";
        for (stage, label) in [
            (
                CreatedExchangeStage::Reload(CreatedReloadSite::RpcLookup),
                "created_exchange_reload",
            ),
            (
                CreatedExchangeStage::PreparePending,
                "created_exchange_prepare_pending",
            ),
            (
                CreatedExchangeStage::AdvancePending,
                "created_exchange_advance_pending",
            ),
            (CreatedExchangeStage::AdvanceAck, "created_exchange"),
        ] {
            let error = anyhow::anyhow!("{private}")
                .context(stage)
                .context(BrokerStep::CreatedExchange);
            let report =
                serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &error)).unwrap();
            assert_eq!(report["broker_step"], label);
            assert_eq!(report["runtime_effect"], "unknown");
            if matches!(stage, CreatedExchangeStage::Reload(_)) {
                assert_eq!(report["created_reload"], json!({"site":"rpc_lookup"}));
            } else {
                assert!(report.get("created_reload").is_none());
            }
            match stage {
                CreatedExchangeStage::AdvancePending => {
                    assert_eq!(report["created_advance"], json!({"stage":"pending"}));
                }
                CreatedExchangeStage::AdvanceAck => {
                    assert_eq!(
                        report["created_advance"],
                        json!({"stage":"acknowledgement"})
                    );
                }
                _ => assert!(report.get("created_advance").is_none()),
            }
            assert!(!report.to_string().contains(private));
        }
    }

    #[test]
    fn created_reload_reports_only_closed_writer_causes() {
        for (failure, cause) in [
            (WriterCallFailure::QueueFull, "queue_full"),
            (WriterCallFailure::ReplyTimeout, "reply_timeout"),
            (WriterCallFailure::Unavailable, "unavailable"),
            (WriterCallFailure::Actor(JournalError::Io), "actor_error"),
        ] {
            let error = anyhow::anyhow!("synthetic private failure")
                .context(failure)
                .context(CreatedExchangeStage::Reload(CreatedReloadSite::CasReload))
                .context(BrokerStep::CreatedExchange);
            let report =
                serde_json::to_value(failure_report(LocalFailurePhaseV1::Worker, &error)).unwrap();
            let mut expected = json!({"site":"cas_reload","cause":cause});
            if cause == "actor_error" {
                expected["journal_code"] = json!("io");
            }
            assert_eq!(report["created_reload"], expected);
            assert_eq!(report["broker_step"], "created_exchange_reload");
            assert_eq!(report["runtime_effect"], "unknown");
            assert!(!report.to_string().contains("synthetic private failure"));
        }
    }

    #[test]
    fn full3_binding_reports_boundary_and_typed_error_without_claiming_effect() {
        let private = "synthetic-private-full3-binding-error";
        for (error, expected) in [
            (
                anyhow::Error::new(EnforcerError::NotReady)
                    .context(Full3BindingStage::ReadNativeBinding),
                json!({"stage":"read_native_binding","enforcer_code":"not_ready"}),
            ),
            (
                anyhow::Error::new(JournalError::UnsafeFilesystem)
                    .context(Full3BindingStage::ReloadJournal),
                json!({"stage":"reload_journal","journal_code":"unsafe_filesystem"}),
            ),
            (
                anyhow::Error::new(JournalError::Io).context(Full3BindingStage::AdvanceWriter),
                json!({"stage":"advance_writer","journal_code":"io"}),
            ),
            (
                anyhow::anyhow!("{private}").context(Full3BindingStage::RetryLimit),
                json!({"stage":"retry_limit","journal_code":"conflict"}),
            ),
        ] {
            let diagnostic = failure_report(LocalFailurePhaseV1::Worker, &error);
            let report = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(report["broker_step"], "full3_binding_publication");
            assert_eq!(report["full3_binding"], expected);
            assert_eq!(report["runtime_effect"], "unknown");
            assert!(!report.to_string().contains(private));
        }
    }

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
    fn negotiated_reservation_refusal_reports_its_closed_motif() {
        for (motif, label) in [
            (
                LegacyReservationMotifV1::ClockRejectedBeforeAllocation,
                "clock_rejected_before_allocation",
            ),
            (
                LegacyReservationMotifV1::ClockRejectedBeforeAck,
                "clock_rejected_before_ack",
            ),
            (
                LegacyReservationMotifV1::InstallDeadlineElapsed,
                "install_deadline_elapsed",
            ),
            (
                LegacyReservationMotifV1::StopReservedBeforeAck,
                "stop_reserved_before_ack",
            ),
        ] {
            let refusal = LegacyReservationRefusalV1::with_motif(EnforcerError::NotReady, motif);
            let error = anyhow::Error::new(ReservationRefusal(refusal))
                .context(ReservationMode::from_replay_only(false))
                .context("synthetic private outer context");
            let report =
                serde_json::to_value(failure_report(LocalFailurePhaseV1::FinalScan, &error))
                    .unwrap();
            assert_eq!(
                report["registration"],
                json!({
                    "stage": "reserve_prospective",
                    "arm_state": "pending",
                    "reservation_mode": "fresh",
                    "enforcer_code": "not_ready",
                    "reservation_motif": label,
                })
            );
            assert!(!report.to_string().contains("synthetic private"));
        }
    }

    #[test]
    fn unnegotiated_reservation_refusal_keeps_motif_absent() {
        let refusal = LegacyReservationRefusalV1::from(EnforcerError::Capacity);
        let error = anyhow::Error::new(ReservationRefusal(refusal))
            .context(ReservationMode::from_replay_only(true));
        let report =
            serde_json::to_value(failure_report(LocalFailurePhaseV1::FinalScan, &error)).unwrap();
        assert_eq!(
            report["registration"],
            json!({
                "stage": "reserve_prospective",
                "arm_state": "pending",
                "reservation_mode": "replay",
                "enforcer_code": "capacity",
            })
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
