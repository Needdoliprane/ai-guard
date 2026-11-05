//! Explicit Kubernetes diagnostics and audited admission dry-run for control.
//! No ambient configuration, credential discovery or workload admission.
use crate::{
    control_fs::PrivateFile,
    model::Event,
    redaction::Redactor,
    runner::{
        AuditClient, ControlIdentityError, kubernetes_preflight::KubernetesPreflightAuditReceiptV1,
    },
    runtime::kubernetes::{
        BeforeKubernetesPreflightGuard, KubernetesAdmissionGapV1, KubernetesAuthorityV1,
        KubernetesCredentials, KubernetesDiagnosticsV1, KubernetesError,
        KubernetesPreflightObservationV1, KubernetesPreflightPlanV1, KubernetesPreflightTargetV1,
        KubernetesRuntime,
    },
};
use clap::{ArgGroup, Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_AUTHORITY_BYTES: u64 = 16 * 1024;
const MAX_CERTIFICATE_BYTES: u64 = 64 * 1024;
const MAX_KEY_BYTES: u64 = 16 * 1024;
const MAX_TOKEN_BYTES: u64 = 8192;
const MAX_CONTROL_BYTES: u64 = 16 * 1024;
const MAX_PLAN_BYTES: u64 = 16 * 1024;
const PREFLIGHT_GUARD_LIFETIME: Duration = Duration::from_secs(60);

#[derive(Args)]
pub struct KubernetesArgs {
    #[command(subcommand)]
    command: KubernetesCommand,
}

#[derive(Subcommand)]
enum KubernetesCommand {
    /// Inspect an explicit Kubernetes authority over HTTPS; never admit work.
    Doctor(DoctorArgs),
    /// Audit and inspect a closed Pod profile using server-side dry-run; never start work.
    Preflight(PreflightArgs),
}

#[derive(Args)]
struct PreflightArgs {
    #[command(flatten)]
    connection: DoctorArgs,
    /// Absolute path to a private closed KubernetesPreflightPlanV1 JSON file.
    #[arg(long, value_name = "FILE")]
    plan_file: PathBuf,
    /// Absolute path to the private gateway/namespace/runner-file configuration.
    #[arg(long, value_name = "FILE")]
    control_file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KubernetesPreflightControlV1 {
    version: u32,
    gateway: String,
    control_namespace_uuid: Uuid,
    runner_token_file: PathBuf,
}

#[derive(Args)]
#[command(group(ArgGroup::new("authentication")
    .required(true)
    .multiple(false)
    .args(["token_file", "client_cert_file"])))]
struct DoctorArgs {
    /// Absolute path to a private KubernetesAuthorityV1 JSON file (16 KiB max).
    #[arg(long, value_name = "FILE")]
    authority_file: PathBuf,
    /// Absolute path to the explicit private PEM CA file (64 KiB max).
    #[arg(long, value_name = "FILE")]
    ca_file: PathBuf,
    /// Absolute path to a private static Bearer token file (8 KiB max).
    #[arg(long, value_name = "FILE", conflicts_with_all = ["client_cert_file", "client_key_file"])]
    token_file: Option<PathBuf>,
    /// Absolute path to the private PEM client chain (64 KiB max).
    #[arg(
        long,
        value_name = "FILE",
        requires = "client_key_file",
        conflicts_with = "token_file"
    )]
    client_cert_file: Option<PathBuf>,
    /// Absolute path to the private PEM client key (16 KiB max).
    #[arg(
        long,
        value_name = "FILE",
        requires = "client_cert_file",
        conflicts_with = "token_file"
    )]
    client_key_file: Option<PathBuf>,
}

/// Closed categories never retain parser, filesystem, credential or HTTP data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesCliError {
    InvalidAuthority,
    InvalidCredentials,
    Inspection(KubernetesError),
    ProtectedOutput,
    OutputUnavailable,
    InvalidPlan,
    InvalidControl,
    ControlIdentity(ControlIdentityError),
    ControlScopeMismatch,
    AuditUnavailable,
    ResultAuditUnavailable,
    PreflightFailed(KubernetesError),
    PreflightReportUnavailable,
    ProtectedPreflightInput,
}
impl std::fmt::Display for KubernetesCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAuthority => f.write_str("Kubernetes authority configuration is invalid or unavailable; use an unchanged private authority file"),
            Self::InvalidCredentials => f.write_str("Kubernetes private credentials or trust are invalid or unavailable; verify the explicit private files"),
            Self::Inspection(error) => std::fmt::Display::fmt(error, f),
            Self::ProtectedOutput => f.write_str("Kubernetes diagnostic output contains protected credential material; inspect the source privately"),
            Self::OutputUnavailable => f.write_str("Kubernetes diagnostic output is unavailable"),
            Self::InvalidPlan => f.write_str("Kubernetes admission preflight plan is invalid or unavailable; use a private closed plan bound to this authority"),
            Self::InvalidControl => f.write_str("Kubernetes preflight control configuration or runner credential is invalid or unavailable; use unchanged private files"),
            Self::ControlIdentity(error) => std::fmt::Display::fmt(error,f),
            Self::ControlScopeMismatch => f.write_str("Kubernetes preflight requires the configured authenticated control namespace and a compatible runner protocol"),
            Self::AuditUnavailable => f.write_str("Kubernetes preflight request audit has no verified durable receipt; reception is unknown, no admission dry-run was requested and no retry was attempted"),
            Self::ResultAuditUnavailable => f.write_str("Kubernetes preflight result audit has no verified durable receipt; admission effect is unknown or possible, no success can be reported and no retry was attempted"),
            Self::PreflightFailed(error) => write!(f,"Kubernetes preflight failed: {error}; admission effect is unknown or possible and no retry was attempted"),
            Self::PreflightReportUnavailable => f.write_str("Kubernetes preflight report is unavailable after result audit; admission effect is possible, no success can be reported and no retry was attempted"),
            Self::ProtectedPreflightInput => f.write_str("Kubernetes preflight configuration contains protected credential material; remove it from authority, image or gateway fields; nothing was transmitted"),
        }
    }
}
impl std::error::Error for KubernetesCliError {}

#[derive(Serialize)]
struct KubernetesDoctorReportV1 {
    version: u32,
    admission_available: bool,
    diagnostics: KubernetesDiagnosticsV1,
    admission_gap_reasons: Vec<AdmissionGapReasonV1>,
}

#[derive(Serialize)]
struct AdmissionGapReasonV1 {
    gap: KubernetesAdmissionGapV1,
    reason: &'static str,
}

fn credential(path: &Path, limit: u64) -> Result<PrivateFile, KubernetesCliError> {
    PrivateFile::open(path, limit).map_err(|_| KubernetesCliError::InvalidCredentials)
}

/// Protect complete retained material and individual PEM lines. Comparing
/// decoded typed values also catches JSON escaping; a collision refuses the
/// report rather than replacing an observed identity with a redacted string.
fn protect(file: &PrivateFile, values: &mut Vec<String>) -> Result<(), KubernetesCliError> {
    let material =
        std::str::from_utf8(file.bytes()).map_err(|_| KubernetesCliError::InvalidCredentials)?;
    values.push(material.to_owned());
    values.extend(material.lines().map(str::trim).map(str::to_owned));
    Ok(())
}

fn control_identity_matches_preflight_scope(
    identity: &crate::control::ControlProtocolV1,
    expected_namespace: Uuid,
) -> bool {
    let expected_namespace = expected_namespace.to_string();
    identity.role == "runner"
        && (3..=8).contains(&identity.schema_version)
        && identity.protocols == [crate::control::SupervisionProtocolV1::BrokerSupervisionV1]
        && identity.control_namespace_uuid.as_deref() == Some(expected_namespace.as_str())
}

pub async fn execute(args: KubernetesArgs) -> Result<(), KubernetesCliError> {
    match args.command {
        KubernetesCommand::Doctor(args) => doctor(args).await,
        KubernetesCommand::Preflight(args) => preflight(args).await,
    }
}

struct LoadedConnection {
    source: PrivateFile,
    runtime: KubernetesRuntime,
    protected: Vec<String>,
}

fn load_connection(args: DoctorArgs) -> Result<LoadedConnection, KubernetesCliError> {
    let source = PrivateFile::open(&args.authority_file, MAX_AUTHORITY_BYTES)
        .map_err(|_| KubernetesCliError::InvalidAuthority)?;
    let authority: KubernetesAuthorityV1 =
        serde_json::from_slice(source.bytes()).map_err(|_| KubernetesCliError::InvalidAuthority)?;
    authority
        .validate()
        .map_err(|_| KubernetesCliError::InvalidAuthority)?;

    let ca = credential(&args.ca_file, MAX_CERTIFICATE_BYTES)?;
    let mut protected = Vec::new();
    protect(&ca, &mut protected)?;
    let credentials = match (args.token_file, args.client_cert_file, args.client_key_file) {
        (Some(path), None, None) => {
            let token = credential(&path, MAX_TOKEN_BYTES)?;
            protect(&token, &mut protected)?;
            KubernetesCredentials::bearer(ca, token)
        }
        (None, Some(certificate), Some(key)) => {
            let certificate = credential(&certificate, MAX_CERTIFICATE_BYTES)?;
            let key = credential(&key, MAX_KEY_BYTES)?;
            protect(&certificate, &mut protected)?;
            protect(&key, &mut protected)?;
            KubernetesCredentials::mutual_tls(ca, certificate, key)
        }
        _ => return Err(KubernetesCliError::InvalidCredentials),
    };
    source
        .revalidate()
        .map_err(|_| KubernetesCliError::InvalidAuthority)?;
    let runtime = KubernetesRuntime::connect(authority, credentials)
        .map_err(KubernetesCliError::Inspection)?;
    Ok(LoadedConnection {
        source,
        runtime,
        protected,
    })
}

async fn doctor(args: DoctorArgs) -> Result<(), KubernetesCliError> {
    let LoadedConnection {
        source,
        runtime,
        protected,
    } = load_connection(args)?;
    let redactor = Redactor::new(protected);
    let diagnostics = runtime
        .doctor()
        .await
        .map_err(KubernetesCliError::Inspection)?;
    let admission_gap_reasons = diagnostics
        .admission_gaps
        .iter()
        .copied()
        .map(|gap| AdmissionGapReasonV1 {
            gap,
            reason: gap.reason(),
        })
        .collect();
    let report = KubernetesDoctorReportV1 {
        version: 1,
        admission_available: false,
        diagnostics,
        admission_gap_reasons,
    };
    let value = serde_json::to_value(&report).map_err(|_| KubernetesCliError::OutputUnavailable)?;
    if redactor.value_unclipped(&value) != value {
        return Err(KubernetesCliError::ProtectedOutput);
    }
    source
        .revalidate()
        .map_err(|_| KubernetesCliError::InvalidAuthority)?;
    let mut bytes =
        serde_json::to_vec(&report).map_err(|_| KubernetesCliError::OutputUnavailable)?;
    bytes.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| KubernetesCliError::OutputUnavailable)
}

/// These typed payloads describe the control's assertion and API receipt,
/// never independent observation or exclusive ownership of a Kube namespace.
#[derive(Serialize)]
struct PreflightRequestedDataV1<'a> {
    version: u32,
    tenant_id: &'a str,
    control_namespace_uuid: &'a str,
    target: &'a KubernetesPreflightTargetV1,
    guard_lifetime_seconds: u64,
    admission_available: bool,
}
#[derive(Serialize)]
struct PreflightResultDataV1<'a> {
    version: u32,
    tenant_id: &'a str,
    control_namespace_uuid: &'a str,
    target: &'a KubernetesPreflightTargetV1,
    request_receipt: &'a KubernetesPreflightAuditReceiptV1,
    outcome: &'static str,
    admission_effect: &'static str,
    emission_guard_consumed: bool,
    runtime_error: Option<&'static str>,
    api_status: Option<u16>,
    observation: Option<&'a KubernetesPreflightObservationV1>,
    admission_available: bool,
}
#[derive(Serialize)]
struct PreflightReportV1<'a> {
    version: u32,
    tenant_id: &'a str,
    control_namespace_uuid: &'a str,
    admission_available: bool,
    request_receipt: &'a KubernetesPreflightAuditReceiptV1,
    result_receipt: &'a KubernetesPreflightAuditReceiptV1,
    observation: &'a KubernetesPreflightObservationV1,
}

struct AuditedPreflightGuard {
    target: KubernetesPreflightTargetV1,
    request_event: Event,
    receipt: KubernetesPreflightAuditReceiptV1,
    files: Vec<PrivateFile>,
    expires_at: Instant,
    consumed: Arc<AtomicBool>,
}
impl BeforeKubernetesPreflightGuard for AuditedPreflightGuard {
    fn check_before_dry_run(
        self: Box<Self>,
        target: &KubernetesPreflightTargetV1,
    ) -> Result<(), KubernetesError> {
        if target != &self.target
            || !self.receipt.matches_event(&self.request_event)
            || self.request_event.id != target.operation_id.to_string()
            || Instant::now() >= self.expires_at
            || self.files.iter().any(|file| file.revalidate().is_err())
        {
            return Err(KubernetesError::PreflightAuthorizationDenied);
        }
        self.consumed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn preflight_event(
    kind: &str,
    target: &KubernetesPreflightTargetV1,
    data: impl Serialize,
) -> Result<Event, KubernetesCliError> {
    let mut event = Event::new(kind);
    if kind.ends_with(".requested") {
        event.id = target.operation_id.to_string();
    }
    event.source = "trusted-control".into();
    event.provenance = "declared".into();
    event.coverage = "partial".into();
    event.session_id = Some(target.session_id.to_string());
    event.correlation_id = Some(target.operation_id.to_string());
    event.data = serde_json::to_value(data).map_err(|_| KubernetesCliError::OutputUnavailable)?;
    Ok(event)
}

fn runtime_failure(error: KubernetesError) -> (&'static str, Option<u16>) {
    use KubernetesError::*;
    (
        match error {
            InvalidAuthority => "invalid_authority",
            InvalidCredentials => "invalid_credentials",
            TransportUnavailable => "transport_unavailable",
            Timeout => "timeout",
            Busy => "busy",
            ResponseTooLarge => "response_too_large",
            ApiRejected(_) => "api_rejected",
            InvalidResponse => "invalid_response",
            AuthorityChanged => "authority_changed",
            PodChanged => "pod_changed",
            InvalidPreflightPlan => "invalid_preflight_plan",
            PreflightAuthorizationDenied => "preflight_authorization_denied",
            PreflightProfileChanged => "preflight_profile_changed",
            WorkloadAdmissionUnavailable => "workload_admission_unavailable",
            HeldCreateAuthorizationDenied => "held_create_authorization_denied",
            HeldCreateControlUnavailable => "held_create_control_unavailable",
            HeldCreateJournalUnavailable => "held_create_journal_unavailable",
        },
        if let ApiRejected(status) = error {
            Some(status)
        } else {
            None
        },
    )
}

async fn preflight(args: PreflightArgs) -> Result<(), KubernetesCliError> {
    let mut connection = load_connection(args.connection)?;
    let plan_source = PrivateFile::open(&args.plan_file, MAX_PLAN_BYTES)
        .map_err(|_| KubernetesCliError::InvalidPlan)?;
    let plan: KubernetesPreflightPlanV1 =
        serde_json::from_slice(plan_source.bytes()).map_err(|_| KubernetesCliError::InvalidPlan)?;
    let target = plan
        .target(connection.runtime.binding())
        .map_err(|_| KubernetesCliError::InvalidPlan)?;
    let control_source = PrivateFile::open(&args.control_file, MAX_CONTROL_BYTES)
        .map_err(|_| KubernetesCliError::InvalidControl)?;
    let control: KubernetesPreflightControlV1 = serde_json::from_slice(control_source.bytes())
        .map_err(|_| KubernetesCliError::InvalidControl)?;
    if control.version != 1
        || control.control_namespace_uuid.is_nil()
        || !control.runner_token_file.is_absolute()
    {
        return Err(KubernetesCliError::InvalidControl);
    }
    let runner_source = PrivateFile::open(&control.runner_token_file, 4096)
        .map_err(|_| KubernetesCliError::InvalidControl)?;
    let runner = std::str::from_utf8(runner_source.bytes())
        .map_err(|_| KubernetesCliError::InvalidControl)?
        .trim();
    protect(&runner_source, &mut connection.protected)
        .map_err(|_| KubernetesCliError::InvalidControl)?;
    connection.protected.push(runner.into());
    let redactor = Redactor::new(connection.protected.clone());
    let audit = AuditClient::new(&control.gateway, runner)
        .map_err(|_| KubernetesCliError::InvalidControl)?
        .with_secrets(connection.protected);
    let input = serde_json::json!({"plan":&plan,"authority":connection.runtime.binding(),"gateway":&control.gateway});
    if redactor.value_unclipped(&input) != input {
        return Err(KubernetesCliError::ProtectedPreflightInput);
    }
    let files = vec![
        connection.source.clone(),
        plan_source,
        control_source.clone(),
        runner_source.clone(),
    ];
    for file in &files {
        file.revalidate()
            .map_err(|_| KubernetesCliError::InvalidControl)?;
    }
    let identity = audit
        .supervision_identity()
        .await
        .map_err(KubernetesCliError::ControlIdentity)?;
    if !control_identity_matches_preflight_scope(&identity, control.control_namespace_uuid) {
        return Err(KubernetesCliError::ControlScopeMismatch);
    }
    let identity_value =
        serde_json::to_value(&identity).map_err(|_| KubernetesCliError::InvalidControl)?;
    if redactor.value_unclipped(&identity_value) != identity_value {
        return Err(KubernetesCliError::ProtectedOutput);
    }
    let namespace = identity
        .control_namespace_uuid
        .as_deref()
        .ok_or(KubernetesCliError::ControlScopeMismatch)?;
    let request_event = preflight_event(
        "runtime.kubernetes.preflight.requested",
        &target,
        PreflightRequestedDataV1 {
            version: 1,
            tenant_id: &identity.tenant_id,
            control_namespace_uuid: namespace,
            target: &target,
            guard_lifetime_seconds: PREFLIGHT_GUARD_LIFETIME.as_secs(),
            admission_available: false,
        },
    )?;
    for file in &files {
        file.revalidate()
            .map_err(|_| KubernetesCliError::InvalidControl)?;
    }
    let request_receipt = audit
        .append_kubernetes_preflight_event(&request_event)
        .await
        .map_err(|_| KubernetesCliError::AuditUnavailable)?;
    let consumed = Arc::new(AtomicBool::new(false));
    let guard = AuditedPreflightGuard {
        target: target.clone(),
        request_event,
        receipt: request_receipt.clone(),
        files: files.clone(),
        expires_at: Instant::now() + PREFLIGHT_GUARD_LIFETIME,
        consumed: consumed.clone(),
    };
    let result = connection.runtime.preflight(&plan, Box::new(guard)).await;
    let (failure, status) = match &result {
        Ok(_) => (None, None),
        Err(error) => {
            let (code, status) = runtime_failure(*error);
            (Some(code), status)
        }
    };
    let result_event = preflight_event(
        "runtime.kubernetes.preflight.result",
        &target,
        PreflightResultDataV1 {
            version: 1,
            tenant_id: &identity.tenant_id,
            control_namespace_uuid: namespace,
            target: &target,
            request_receipt: &request_receipt,
            outcome: if result.is_ok() { "accepted" } else { "error" },
            admission_effect: if result.is_ok() {
                "dry_run_response_validated"
            } else {
                "unknown_possible"
            },
            emission_guard_consumed: consumed.load(Ordering::SeqCst),
            runtime_error: failure,
            api_status: status,
            observation: result.as_ref().ok(),
            admission_available: false,
        },
    )
    .map_err(|_| KubernetesCliError::ResultAuditUnavailable)?;
    // Even a runtime error requires a result attempt. Replaced control files
    // prevent sending with a credential that the operator may have revoked.
    control_source
        .revalidate()
        .and_then(|()| runner_source.revalidate())
        .map_err(|_| KubernetesCliError::ResultAuditUnavailable)?;
    let result_receipt = audit
        .append_kubernetes_preflight_event(&result_event)
        .await
        .map_err(|_| KubernetesCliError::ResultAuditUnavailable)?;
    let observation = result.map_err(KubernetesCliError::PreflightFailed)?;
    for file in &files {
        file.revalidate()
            .map_err(|_| KubernetesCliError::PreflightReportUnavailable)?;
    }
    let report = PreflightReportV1 {
        version: 1,
        tenant_id: &identity.tenant_id,
        control_namespace_uuid: namespace,
        admission_available: false,
        request_receipt: &request_receipt,
        result_receipt: &result_receipt,
        observation: &observation,
    };
    let value = serde_json::to_value(&report)
        .map_err(|_| KubernetesCliError::PreflightReportUnavailable)?;
    if redactor.value_unclipped(&value) != value {
        return Err(KubernetesCliError::PreflightReportUnavailable);
    }
    let mut bytes =
        serde_json::to_vec(&report).map_err(|_| KubernetesCliError::PreflightReportUnavailable)?;
    bytes.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| KubernetesCliError::PreflightReportUnavailable)
}

#[cfg(test)]
mod identity_scope_tests {
    use super::control_identity_matches_preflight_scope;
    use crate::control::{ControlProtocolV1, SupervisionProtocolV1};
    use uuid::Uuid;

    fn protocol(
        schema_version: u32,
        role: &str,
        namespace: Option<&str>,
        protocols: Vec<SupervisionProtocolV1>,
    ) -> ControlProtocolV1 {
        ControlProtocolV1 {
            version: 1,
            schema_version,
            control_namespace_uuid: namespace.map(str::to_owned),
            tenant_id: "tenant-a".into(),
            role: role.into(),
            protocols,
            active: false,
        }
    }

    #[test]
    fn schema8_runner_with_exact_protocol_and_namespace_passes_preflight_scope() {
        let expected_namespace = Uuid::from_u128(3);
        let identity = protocol(
            8,
            "runner",
            Some(&expected_namespace.to_string()),
            vec![SupervisionProtocolV1::BrokerSupervisionV1],
        );

        assert!(control_identity_matches_preflight_scope(
            &identity,
            expected_namespace
        ));
    }

    #[test]
    fn preflight_scope_rejects_unknown_versions_roles_protocols_and_namespaces() {
        let expected_namespace = Uuid::from_u128(3);
        let expected_text = expected_namespace.to_string();
        let broker_protocol = SupervisionProtocolV1::BrokerSupervisionV1;
        let invalid = [
            (
                "future schema",
                protocol(9, "runner", Some(&expected_text), vec![broker_protocol]),
            ),
            (
                "unknown schema",
                protocol(
                    u32::MAX,
                    "runner",
                    Some(&expected_text),
                    vec![broker_protocol],
                ),
            ),
            (
                "non-runner role",
                protocol(8, "viewer", Some(&expected_text), vec![broker_protocol]),
            ),
            (
                "missing protocol",
                protocol(8, "runner", Some(&expected_text), vec![]),
            ),
            (
                "duplicate protocol",
                protocol(
                    8,
                    "runner",
                    Some(&expected_text),
                    vec![broker_protocol, broker_protocol],
                ),
            ),
            (
                "different namespace",
                protocol(
                    8,
                    "runner",
                    Some("44444444-4444-4444-8444-444444444444"),
                    vec![broker_protocol],
                ),
            ),
            (
                "missing namespace",
                protocol(8, "runner", None, vec![broker_protocol]),
            ),
        ];

        for (label, identity) in invalid {
            assert!(
                !control_identity_matches_preflight_scope(&identity, expected_namespace),
                "accepted {label}"
            );
        }
    }
}
