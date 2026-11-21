//! Offline aggregation of explicitly selected, redacted provider recipe reports.
use agentsandbox::provider::AuthMode;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::ExitCode,
};
use uuid::Uuid;

const MAX_REPORT_BYTES: u64 = 256 * 1024;
const MAX_SAFE_JS_INTEGER: u64 = (1 << 53) - 1;

#[derive(Parser)]
#[command(
    about = "Summarize existing provider and browser reports without running a provider or engine"
)]
struct Args {
    /// Exact case-sensitive host value from the report, such as Darwin/arm64.
    #[arg(long, value_parser = host_platform)]
    host: String,
    #[arg(long, value_enum)]
    runtime: Runtime,
    /// Select the eight legacy cases or the four supervised batch cases.
    #[arg(long, value_enum, default_value = "legacy")]
    execution_path: ExecutionPath,
    #[arg(long, value_parser = sha256_arg)]
    cli_sha256: String,
    /// Local immutable image ID, as a SHA-256 with optional sha256: prefix.
    #[arg(long, value_parser = sha256_arg)]
    worker_image_sha256: String,
    /// Whether the runtime exception was requested. Required for Docker and Podman.
    #[arg(long, value_enum)]
    rootful_exception: Option<Rootful>,
    /// CLI result.json or its redacted archived copy. Repeat for each case.
    #[arg(long, value_name = "JSON")]
    cli_report: Vec<PathBuf>,
    /// Standalone browser session trace. Repeat for cases without an embedded trace.
    #[arg(long, value_name = "JSON")]
    browser_report: Vec<PathBuf>,
    /// Exit with status 1 after writing the matrix unless all selected cases pass.
    #[arg(long)]
    require_pass: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Rootful {
    #[value(name = "true")]
    Requested,
    #[value(name = "false")]
    NotRequested,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum Runtime {
    Docker,
    Podman,
    #[serde(alias = "k8s")]
    #[value(alias = "k8s")]
    Kubernetes,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Client {
    Codex,
    #[serde(alias = "claude_code")]
    ClaudeCode,
}

impl Client {
    fn model(self) -> &'static str {
        match self {
            Self::Codex => "gpt-6-luna",
            Self::ClaudeCode => "claude-haiku-5-5",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Transport {
    Batch,
    Tty,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum ExecutionPath {
    #[default]
    Legacy,
    BrokerBatch,
}

impl ExecutionPath {
    fn is_legacy(&self) -> bool {
        *self == Self::Legacy
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Status {
    Pass,
    Fail,
    Blocked,
    NotRun,
}

#[derive(Debug, Serialize)]
struct Candidate {
    host: String,
    runtime: Runtime,
    #[serde(skip_serializing_if = "ExecutionPath::is_legacy")]
    execution_path: ExecutionPath,
    cli_sha256: String,
    worker_image_sha256: String,
    rootful_exception_requested: Option<bool>,
}

impl Args {
    fn candidate(&self) -> Result<Candidate, &'static str> {
        if self.execution_path == ExecutionPath::BrokerBatch && self.runtime == Runtime::Kubernetes
        {
            return Err("--execution-path broker-batch supports Docker and Podman only");
        }
        let rootful_exception_requested = match (self.runtime, self.rootful_exception) {
            (Runtime::Kubernetes, None) => None,
            (Runtime::Kubernetes, Some(_)) => {
                return Err("--rootful-exception applies only to Docker and Podman");
            }
            (_, None) => {
                return Err("select --rootful-exception true or false for Docker and Podman");
            }
            (_, Some(requested)) => Some(matches!(requested, Rootful::Requested)),
        };
        Ok(Candidate {
            host: self.host.clone(),
            runtime: self.runtime,
            execution_path: self.execution_path,
            cli_sha256: self.cli_sha256.clone(),
            worker_image_sha256: format!("sha256:{}", self.worker_image_sha256),
            rootful_exception_requested,
        })
    }
}

fn host_platform(value: &str) -> Result<String, &'static str> {
    let valid = |part: &str| {
        !part.is_empty()
            && part.len() <= 32
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    if value
        .split_once('/')
        .is_some_and(|(os, arch)| valid(os) && valid(arch))
    {
        Ok(value.to_owned())
    } else {
        Err("expected an explicit OS/architecture, such as Darwin/arm64")
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_arg(value: &str) -> Result<String, &'static str> {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    is_sha256(hex)
        .then(|| hex.to_owned())
        .ok_or("expected 64 lowercase hexadecimal SHA-256 digits")
}

#[derive(Deserialize)]
struct Audit {
    valid: bool,
    event_count: u64,
    head_hash: String,
}

#[derive(Deserialize)]
struct TtyProof {
    stage: String,
    receipts: u32,
    cli_exit_status: Option<i32>,
    termios_restored: bool,
}

#[derive(Deserialize)]
struct TtyTerminalFacts {
    exit_code: Option<i64>,
    oom_killed: Option<bool>,
    cleanup_removed: Option<bool>,
    timed_out: Option<bool>,
    transport_stop_reason: Option<String>,
    control_deadline: Option<serde_json::Value>,
}

impl TtyTerminalFacts {
    fn clean(&self) -> bool {
        self.exit_code == Some(0)
            && self.oom_killed != Some(true)
            && self.cleanup_removed == Some(true)
            && self.timed_out == Some(false)
            && self.transport_stop_reason.is_none()
            && self.control_deadline.is_none()
    }
}

#[derive(Deserialize)]
struct CliReport {
    status: Status,
    stage: String,
    host: Option<String>,
    runtime: Runtime,
    #[serde(default)]
    execution_path: ExecutionPath,
    cli_sha256: Option<String>,
    image_requested: Option<String>,
    worker_image_id: Option<String>,
    rootful_exception_requested: Option<bool>,
    client: Client,
    auth_mode: AuthMode,
    model_requested: String,
    transport: Option<Transport>,
    session_id: Option<Uuid>,
    worker_id: Option<Uuid>,
    authority_id: Option<Uuid>,
    profile: Option<String>,
    cli_exit_status: Option<i32>,
    runtime_result: Option<String>,
    quota_released: Option<bool>,
    cleanup: Option<String>,
    original_unchanged: Option<bool>,
    result_sha256: Option<String>,
    audit_before: Option<Audit>,
    audit_after: Option<Audit>,
    tty: Option<TtyProof>,
    tty_terminal_result: Option<TtyTerminalFacts>,
    new_checkpoint_count: Option<u32>,
    browser: Option<BrowserFacts>,
    broker_operation_id: Option<Uuid>,
    broker_terminal: Option<BrokerTerminal>,
    broker_export_command_id: Option<Uuid>,
    broker_export_request_id: Option<Uuid>,
    broker_export_manifest_sha256: Option<String>,
    broker_run_reply: Option<String>,
    broker_cli_proof: Option<String>,
    broker_browser: Option<BrowserReport>,
}

#[derive(Deserialize)]
struct BrokerTerminal {
    session_id: Uuid,
    container_id: String,
    manifest_digest: String,
    result_audit_seq: u64,
    scan_audit_seq: u64,
    cleanup_audit_seq: u64,
}

#[derive(Deserialize)]
struct BrowserReport {
    status: Status,
    mode: Option<String>,
    #[serde(default, rename = "executionPath", alias = "execution_path")]
    execution_path: ExecutionPath,
    transport: Option<Transport>,
    client: Option<Client>,
    #[serde(rename = "authMode")]
    auth_mode: Option<AuthMode>,
    #[serde(rename = "requestedModel")]
    model_requested: Option<String>,
    runtime: Option<Runtime>,
    #[serde(rename = "workerOutcome")]
    worker_outcome: Option<String>,
    #[serde(rename = "terminalKind")]
    terminal_kind: Option<String>,
    #[serde(rename = "terminalExitCode")]
    terminal_exit_code: Option<i32>,
    #[serde(flatten)]
    facts: BrowserFacts,
    #[serde(flatten)]
    broker: BrokerBrowserFacts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerBrowserFacts {
    operation_id: Option<Uuid>,
    authority_id: Option<Uuid>,
    worker_image_id: Option<String>,
    container_id: Option<String>,
    client_profile: Option<String>,
    export_request_id: Option<Uuid>,
    scan_manifest_sha256: Option<String>,
    scan_event_id: Option<Uuid>,
    cleanup_event_id: Option<Uuid>,
    decision_audit_seq: Option<u64>,
    result_audit_seq: Option<u64>,
    scan_audit_seq: Option<u64>,
    cleanup_audit_seq: Option<u64>,
    export_audit_seq: Option<u64>,
}

#[derive(Deserialize)]
struct BrowserFacts {
    #[serde(alias = "session")]
    session_id: Option<Uuid>,
    #[serde(alias = "workerId")]
    worker_id: Option<Uuid>,
    #[serde(rename = "decisionEventId")]
    decision_event_id: Option<Uuid>,
    #[serde(rename = "terminalEventId")]
    terminal_event_id: Option<Uuid>,
    #[serde(rename = "exportEventId")]
    export_event_id: Option<Uuid>,
    #[serde(rename = "fileEventId")]
    file_event_id: Option<Uuid>,
    #[serde(alias = "fileSha256")]
    file_sha256: Option<String>,
    #[serde(alias = "exportManifestSha256")]
    export_manifest_sha256: Option<String>,
    #[serde(alias = "timeoutLabelsVerified")]
    timeout_labels_verified: Option<bool>,
    #[serde(alias = "auditExportSha256")]
    audit_export_sha256: Option<String>,
    #[serde(alias = "count")]
    audit_export_event_count: Option<u64>,
    #[serde(alias = "browser")]
    chromium_version: Option<String>,
}

#[derive(Serialize)]
struct Report<T> {
    input: usize,
    #[serde(flatten)]
    content: T,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Source {
    Cli,
    Browser,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Rejection {
    UnreadableReport,
    InvalidReport,
    OtherCandidate,
    OtherExecutionPath,
    MissingCandidateIdentity,
    RequestedModelDoesNotMatch,
    UnknownTransport,
    IncompatibleExecutionPathTransport,
    DuplicateCliCaseOrSession,
    UnmatchedBrowserSessionOrAxes,
    DuplicateBrowserTraces,
    IncompleteBrowserTrace,
}

#[derive(Serialize)]
struct Exclusion {
    source: Source,
    input: usize,
    reason: Rejection,
    reported_status: Option<Status>,
}

fn exclude(
    excluded: &mut Vec<Exclusion>,
    source: Source,
    input: usize,
    reason: Rejection,
    reported_status: Option<Status>,
) {
    excluded.push(Exclusion {
        source,
        input,
        reason,
        reported_status,
    });
}

fn read_report<T: DeserializeOwned>(path: &Path) -> Result<T, Rejection> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Rejection::UnreadableReport)?;
    if !file
        .metadata()
        .map_err(|_| Rejection::UnreadableReport)?
        .is_file()
    {
        return Err(Rejection::UnreadableReport);
    }
    let mut bytes = Vec::new();
    file.take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Rejection::UnreadableReport)?;
    if bytes.len() as u64 > MAX_REPORT_BYTES {
        return Err(Rejection::InvalidReport);
    }
    serde_json::from_slice(&bytes).map_err(|_| Rejection::InvalidReport)
}

fn read_reports<T: DeserializeOwned>(
    files: &[PathBuf],
    source: Source,
    excluded: &mut Vec<Exclusion>,
) -> Vec<Report<T>> {
    files
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let input = index + 1;
            match read_report(path) {
                Ok(content) => Some(Report { input, content }),
                Err(reason) => {
                    exclude(excluded, source, input, reason, None);
                    None
                }
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
struct CaseKey {
    client: Client,
    auth_mode: AuthMode,
    transport: Transport,
}

#[derive(Serialize)]
struct Case {
    #[serde(flatten)]
    key: CaseKey,
    status: Status,
    model_requested: &'static str,
    cli_status: Status,
    cli_input: Option<usize>,
    cli_session_id: Option<Uuid>,
    cli_reason: Option<&'static str>,
    browser_status: Status,
    browser: Option<BrowserTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    broker_browser: Option<BrokerBrowserTrace>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BrowserScope {
    SessionTrace,
    SessionTimeoutTrace,
}

#[derive(Serialize)]
struct BrowserTrace {
    source: Source,
    input: usize,
    scope: BrowserScope,
    session_id: Uuid,
    worker_id: Option<Uuid>,
    #[serde(flatten)]
    key: CaseKey,
    model_requested: &'static str,
    runtime: Runtime,
    decision_event_id: Uuid,
    terminal_event_id: Uuid,
    export_event_id: Option<Uuid>,
    file_event_id: Option<Uuid>,
    file_sha256: Option<String>,
    export_manifest_sha256: Option<String>,
    timeout_labels_verified: Option<bool>,
    audit_export_sha256: String,
    audit_export_event_count: u64,
    chromium_version: Option<String>,
    bundle_sha256: Option<String>,
}

#[derive(Serialize)]
struct BrokerBrowserTrace {
    source: Source,
    input: usize,
    operation_id: Uuid,
    session_id: Uuid,
    worker_id: Uuid,
    authority_id: Uuid,
    worker_image_id: String,
    container_id: String,
    export_request_id: Uuid,
    file_sha256: String,
    scan_manifest_sha256: String,
    export_manifest_sha256: String,
    decision_event_id: Uuid,
    terminal_event_id: Uuid,
    scan_event_id: Uuid,
    cleanup_event_id: Uuid,
    export_event_id: Uuid,
    decision_audit_seq: u64,
    result_audit_seq: u64,
    scan_audit_seq: u64,
    cleanup_audit_seq: u64,
    export_audit_seq: u64,
    audit_export_sha256: String,
    audit_export_event_count: u64,
    chromium_version: String,
}

#[derive(Serialize)]
struct Matrix {
    version: u32,
    candidate: Candidate,
    image_identity_source: &'static str,
    model_served: Option<String>,
    client_version: Option<String>,
    release_qualification: bool,
    cases: Vec<Case>,
    excluded: Vec<Exclusion>,
}

fn check_candidate(cli: &CliReport, candidate: &Candidate) -> Result<(), Rejection> {
    if cli.execution_path != candidate.execution_path {
        return Err(Rejection::OtherExecutionPath);
    }
    let host = cli
        .host
        .as_deref()
        .filter(|value| host_platform(value).is_ok());
    let binary = cli.cli_sha256.as_deref().filter(|value| is_sha256(value));
    let image = cli
        .image_requested
        .as_deref()
        .filter(|value| value.strip_prefix("sha256:").is_some_and(is_sha256));
    let rootful = cli.rootful_exception_requested;
    if cli.runtime != candidate.runtime
        || host.is_some_and(|host| host != candidate.host)
        || binary.is_some_and(|binary| binary != candidate.cli_sha256)
        || image.is_some_and(|image| image != candidate.worker_image_sha256)
        || rootful.is_some_and(|rootful| Some(rootful) != candidate.rootful_exception_requested)
    {
        return Err(Rejection::OtherCandidate);
    }
    if host.is_none()
        || binary.is_none()
        || image.is_none()
        || rootful != candidate.rootful_exception_requested
    {
        return Err(Rejection::MissingCandidateIdentity);
    }
    if cli.model_requested != cli.client.model() {
        return Err(Rejection::RequestedModelDoesNotMatch);
    }
    Ok(())
}

impl CaseKey {
    fn from_cli(cli: &CliReport, browsers: &[Report<BrowserReport>]) -> Result<Self, Rejection> {
        let mut key = Self {
            client: cli.client,
            auth_mode: cli.auth_mode,
            transport: Transport::Batch,
        };
        if cli.execution_path == ExecutionPath::BrokerBatch {
            return match (cli.transport, cli.tty.is_some()) {
                (Some(Transport::Batch), false) => Ok(key),
                (None, false) => Err(Rejection::UnknownTransport),
                _ => Err(Rejection::IncompatibleExecutionPathTransport),
            };
        }
        key.transport = if let Some(transport) = cli.transport {
            transport
        } else if cli.tty.is_none()
            && browsers.iter().any(|browser| {
                browser.content.matches(cli, key)
                    && browser.content.scope() == Some(BrowserScope::SessionTrace)
                    && browser.content.facts.valid_for(
                        cli,
                        Transport::Batch,
                        BrowserScope::SessionTrace,
                    )
            })
        {
            Transport::Batch
        } else {
            return Err(Rejection::UnknownTransport);
        };
        Ok(key)
    }
}

impl CliReport {
    fn complete(&self, transport: Transport) -> bool {
        self.session_id.is_some_and(|id| !id.is_nil())
            && self.worker_id.is_none_or(|id| !id.is_nil())
            && self.cli_exit_status == Some(0)
            && self.runtime_result.as_deref() == Some("terminal")
            && self.quota_released == Some(true)
            && self.cleanup.as_deref() == Some("removed")
            && self.original_unchanged == Some(true)
            && self
                .audit_before
                .as_ref()
                .zip(self.audit_after.as_ref())
                .is_some_and(|(before, after)| {
                    before.valid
                        && after.valid
                        && is_sha256(&before.head_hash)
                        && is_sha256(&after.head_hash)
                        && after.event_count > before.event_count
                        && before.head_hash != after.head_hash
                })
            && match transport {
                Transport::Batch => {
                    self.tty.is_none() && self.result_sha256.as_deref().is_some_and(is_sha256)
                }
                Transport::Tty => {
                    self.result_sha256.is_none()
                        && self.new_checkpoint_count == Some(1)
                        && self
                            .tty_terminal_result
                            .as_ref()
                            .is_none_or(TtyTerminalFacts::clean)
                        && self.tty.as_ref().is_some_and(|tty| {
                            tty.stage == "complete"
                                && tty.receipts == 2
                                && tty.cli_exit_status == Some(0)
                                && tty.termios_restored
                        })
                }
            }
    }

    fn verdict(&self, transport: Transport) -> (Status, Option<&'static str>) {
        match (self.status, self.stage.as_str()) {
            (Status::Pass, "complete") | (Status::Fail | Status::Blocked, "browser")
                if self.complete(transport) =>
            {
                if self.worker_image_id == self.image_requested {
                    (Status::Pass, None)
                } else {
                    (Status::Fail, Some("worker_image_id_not_confirmed"))
                }
            }
            (Status::Pass, _) => (
                Status::Fail,
                Some("missing_terminal_quota_export_audit_or_transport_proof"),
            ),
            (status, _) => (status, None),
        }
    }

    fn broker_verdict(&self) -> (Status, Option<&'static str>) {
        match (self.status, self.stage.as_str()) {
            (Status::Pass, "complete") | (Status::Fail | Status::Blocked, "broker_browser")
                if self.complete(Transport::Batch) && self.broker_result_is_correlated() =>
            {
                (Status::Pass, None)
            }
            (Status::Pass, _) | (Status::Blocked, "broker_browser") => {
                (Status::Blocked, Some("missing_broker_cli_evidence"))
            }
            (status, _) => (status, None),
        }
    }

    fn broker_result_is_correlated(&self) -> bool {
        self.broker_run_reply.as_deref() == Some("matched")
            && self.broker_cli_proof.as_deref() == Some("terminal_and_export_verified")
            && self.broker_operation_id.is_some_and(|id| !id.is_nil())
            && self
                .broker_export_command_id
                .is_some_and(|id| !id.is_nil() && self.broker_export_request_id == Some(id))
            && self
                .broker_export_manifest_sha256
                .as_deref()
                .is_some_and(is_sha256)
            && self
                .profile
                .as_ref()
                .is_some_and(|profile| !profile.is_empty())
            && self.authority_id.is_none_or(|id| !id.is_nil())
            && self.broker_terminal.as_ref().is_some_and(|terminal| {
                self.session_id == Some(terminal.session_id)
                    && is_sha256(&terminal.container_id)
                    && is_sha256(&terminal.manifest_digest)
                    && positive_distinct(&[
                        terminal.result_audit_seq,
                        terminal.scan_audit_seq,
                        terminal.cleanup_audit_seq,
                    ])
            })
    }
}

fn positive_distinct(sequences: &[u64]) -> bool {
    sequences
        .iter()
        .enumerate()
        .all(|(index, seq)| *seq > 0 && !sequences[..index].contains(seq))
}

impl BrowserReport {
    fn matches(&self, cli: &CliReport, key: CaseKey) -> bool {
        self.execution_path == cli.execution_path
            && cli.session_id.is_some_and(|id| !id.is_nil())
            && self.facts.session_id == cli.session_id
            && self.client == Some(key.client)
            && self.auth_mode == Some(key.auth_mode)
            && self.transport == Some(key.transport)
            && self.runtime == Some(cli.runtime)
            && self.model_requested.as_deref() == Some(key.client.model())
    }

    fn scope(&self) -> Option<BrowserScope> {
        match (self.status, self.mode.as_deref()) {
            (Status::Pass, Some("session_trace")) => Some(BrowserScope::SessionTrace),
            (Status::Pass, Some("session_timeout_trace"))
                if self.transport == Some(Transport::Tty)
                    && self.worker_outcome.as_deref() == Some("timeout")
                    && self.terminal_kind.as_deref() == Some("command.failed")
                    && self.terminal_exit_code.is_some_and(|code| code != 0) =>
            {
                Some(BrowserScope::SessionTimeoutTrace)
            }
            _ => None,
        }
    }

    fn broker_trace(
        &self,
        cli: &CliReport,
        key: CaseKey,
        source: Source,
        input: usize,
    ) -> Option<BrokerBrowserTrace> {
        if self.status != Status::Pass
            || self.mode.as_deref() != Some("broker_session_trace")
            || !self.matches(cli, key)
        {
            return None;
        }
        let terminal = cli.broker_terminal.as_ref()?;
        let broker = &self.broker;
        let facts = &self.facts;
        let operation_id = broker.operation_id?;
        let session_id = facts.session_id?;
        let worker_id = facts.worker_id?;
        let authority_id = broker.authority_id?;
        let worker_image_id = broker.worker_image_id.as_ref()?;
        let container_id = broker.container_id.as_ref()?;
        let export_request_id = broker.export_request_id?;
        let file_sha256 = facts.file_sha256.as_ref()?;
        let scan_manifest_sha256 = broker.scan_manifest_sha256.as_ref()?;
        let export_manifest_sha256 = facts.export_manifest_sha256.as_ref()?;
        let ids = [
            facts.decision_event_id?,
            facts.terminal_event_id?,
            broker.scan_event_id?,
            broker.cleanup_event_id?,
            facts.export_event_id?,
        ];
        let sequences = [
            broker.decision_audit_seq?,
            broker.result_audit_seq?,
            broker.cleanup_audit_seq?,
            broker.scan_audit_seq?,
            broker.export_audit_seq?,
        ];
        let audit_export_event_count = facts.audit_export_event_count?;
        let audit_export_sha256 = facts.audit_export_sha256.as_ref()?;
        if operation_id.is_nil()
            || cli.broker_operation_id != Some(operation_id)
            || terminal.session_id != session_id
            || worker_id.is_nil()
            || cli.worker_id.is_some_and(|id| id != worker_id)
            || authority_id.is_nil()
            || cli.authority_id.is_some_and(|id| id != authority_id)
            || cli.image_requested.as_ref() != Some(worker_image_id)
            || cli
                .worker_image_id
                .as_ref()
                .is_some_and(|id| id != worker_image_id)
            || container_id != &terminal.container_id
            || cli.profile.is_none()
            || cli.profile != broker.client_profile
            || cli.broker_export_request_id != Some(export_request_id)
            || cli.result_sha256.as_ref() != Some(file_sha256)
            || scan_manifest_sha256 != &terminal.manifest_digest
            || cli.broker_export_manifest_sha256.as_ref() != Some(export_manifest_sha256)
            || ids
                .iter()
                .enumerate()
                .any(|(i, id)| id.is_nil() || ids[..i].contains(id))
            || sequences[0] == 0
            || sequences[4] > MAX_SAFE_JS_INTEGER
            || !sequences.windows(2).all(|pair| pair[0] < pair[1])
            || sequences[1..4]
                != [
                    terminal.result_audit_seq,
                    terminal.cleanup_audit_seq,
                    terminal.scan_audit_seq,
                ]
            || !is_sha256(audit_export_sha256)
            || audit_export_event_count < 5
        {
            return None;
        }
        let [
            decision_event_id,
            terminal_event_id,
            scan_event_id,
            cleanup_event_id,
            export_event_id,
        ] = ids;
        let [
            decision_audit_seq,
            result_audit_seq,
            cleanup_audit_seq,
            scan_audit_seq,
            export_audit_seq,
        ] = sequences;
        Some(BrokerBrowserTrace {
            source,
            input,
            operation_id,
            session_id,
            worker_id,
            authority_id,
            worker_image_id: worker_image_id.clone(),
            container_id: container_id.clone(),
            export_request_id,
            file_sha256: file_sha256.clone(),
            scan_manifest_sha256: scan_manifest_sha256.clone(),
            export_manifest_sha256: export_manifest_sha256.clone(),
            decision_event_id,
            terminal_event_id,
            scan_event_id,
            cleanup_event_id,
            export_event_id,
            decision_audit_seq,
            result_audit_seq,
            scan_audit_seq,
            cleanup_audit_seq,
            export_audit_seq,
            audit_export_sha256: audit_export_sha256.clone(),
            audit_export_event_count,
            chromium_version: chromium_version(facts.chromium_version.as_deref())?.to_owned(),
        })
    }
}

fn chromium_version(value: Option<&str>) -> Option<&str> {
    value.filter(|version| {
        let parts = version.split('.').collect::<Vec<_>>();
        (2..=4).contains(&parts.len())
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 6
                    && part.bytes().all(|byte| byte.is_ascii_digit())
            })
    })
}

impl BrowserFacts {
    fn export_manifest_matches(&self) -> bool {
        match (self.export_event_id, self.export_manifest_sha256.as_deref()) {
            (Some(_), Some(hash)) => is_sha256(hash),
            (Some(_), None) => self.worker_id.is_none(), // Historical browser records lacked both fields.
            (None, None) => true,
            (None, Some(_)) => false,
        }
    }

    fn valid_for(&self, cli: &CliReport, transport: Transport, scope: BrowserScope) -> bool {
        let ids = [
            self.decision_event_id,
            self.terminal_event_id,
            self.export_event_id,
            self.file_event_id,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        self.session_id.is_some_and(|id| !id.is_nil())
            && self.session_id == cli.session_id
            && self.worker_id.is_none_or(|id| !id.is_nil())
            && cli.worker_id.is_none_or(|id| Some(id) == self.worker_id)
            && self.export_manifest_matches()
            && (self.timeout_labels_verified.is_none()
                || scope == BrowserScope::SessionTimeoutTrace)
            && self.decision_event_id.is_some()
            && self.terminal_event_id.is_some()
            && self.audit_export_sha256.as_deref().is_some_and(is_sha256)
            && self
                .audit_export_event_count
                .is_some_and(|count| count >= ids.len() as u64)
            && ids
                .iter()
                .enumerate()
                .all(|(index, id)| !id.is_nil() && !ids[..index].contains(id))
            && match (transport, scope) {
                (Transport::Batch, BrowserScope::SessionTrace) => {
                    self.export_event_id.is_some()
                        && self.file_event_id.is_some()
                        && self.file_sha256.as_deref().is_some_and(is_sha256)
                        && self.file_sha256 == cli.result_sha256
                }
                (Transport::Tty, BrowserScope::SessionTrace) => {
                    self.export_event_id.is_some()
                        && self.file_event_id.is_none()
                        && self.file_sha256.is_none()
                }
                (Transport::Tty, BrowserScope::SessionTimeoutTrace) => {
                    self.file_event_id.is_none() && self.file_sha256.is_none()
                }
                (Transport::Batch, BrowserScope::SessionTimeoutTrace) => false,
            }
    }

    fn trace(
        &self,
        cli: &CliReport,
        key: CaseKey,
        scope: BrowserScope,
        source: Source,
        input: usize,
    ) -> Option<BrowserTrace> {
        if !self.valid_for(cli, key.transport, scope) {
            return None;
        }
        Some(BrowserTrace {
            source,
            input,
            scope,
            key,
            runtime: cli.runtime,
            model_requested: key.client.model(),
            session_id: self.session_id?,
            worker_id: self.worker_id,
            decision_event_id: self.decision_event_id?,
            terminal_event_id: self.terminal_event_id?,
            export_event_id: self.export_event_id,
            file_event_id: self.file_event_id,
            file_sha256: self.file_sha256.clone(),
            export_manifest_sha256: self.export_manifest_sha256.clone(),
            timeout_labels_verified: self.timeout_labels_verified,
            audit_export_sha256: self.audit_export_sha256.clone()?,
            audit_export_event_count: self.audit_export_event_count?,
            chromium_version: chromium_version(self.chromium_version.as_deref()).map(str::to_owned),
            bundle_sha256: None,
        })
    }
}

fn browser_verdict(
    cli: &Report<CliReport>,
    key: CaseKey,
    browsers: &[Report<BrowserReport>],
    consumed: &mut Vec<usize>,
    excluded: &mut Vec<Exclusion>,
) -> (Status, Option<BrowserTrace>) {
    let report = &cli.content;
    let external: Vec<_> = browsers
        .iter()
        .filter(|browser| browser.content.matches(report, key))
        .collect();
    consumed.extend(external.iter().map(|browser| browser.input));
    if external.len() + usize::from(report.browser.is_some()) > 1 {
        for browser in external {
            exclude(
                excluded,
                Source::Browser,
                browser.input,
                Rejection::DuplicateBrowserTraces,
                Some(browser.content.status),
            );
        }
        if report.browser.is_some() {
            exclude(
                excluded,
                Source::Cli,
                cli.input,
                Rejection::DuplicateBrowserTraces,
                Some(report.status),
            );
        }
        return (Status::Blocked, None);
    }
    if let Some(browser) = external.first() {
        if browser.content.status != Status::Pass {
            return (browser.content.status, None);
        }
        let trace = browser.content.scope().and_then(|scope| {
            browser
                .content
                .facts
                .trace(report, key, scope, Source::Browser, browser.input)
        });
        if trace.is_none() {
            exclude(
                excluded,
                Source::Browser,
                browser.input,
                Rejection::IncompleteBrowserTrace,
                Some(browser.content.status),
            );
            return (Status::Fail, None);
        }
        return (Status::Pass, trace);
    }
    if let Some(facts) = &report.browser {
        let trace = (report.status == Status::Pass && report.stage == "complete")
            .then(|| {
                facts.trace(
                    report,
                    key,
                    BrowserScope::SessionTrace,
                    Source::Cli,
                    cli.input,
                )
            })
            .flatten();
        if trace.is_none() {
            exclude(
                excluded,
                Source::Cli,
                cli.input,
                Rejection::IncompleteBrowserTrace,
                Some(report.status),
            );
            return (Status::Fail, None);
        }
        return (Status::Pass, trace);
    }
    match (report.status, report.stage.as_str()) {
        (status @ (Status::Fail | Status::Blocked), "browser" | "browser_preflight") => {
            (status, None)
        }
        _ => (Status::NotRun, None),
    }
}

fn broker_browser_verdict(
    cli: &Report<CliReport>,
    key: CaseKey,
    browsers: &[Report<BrowserReport>],
    consumed: &mut Vec<usize>,
    excluded: &mut Vec<Exclusion>,
) -> (Status, Option<BrokerBrowserTrace>) {
    let report = &cli.content;
    let external: Vec<_> = browsers
        .iter()
        .filter(|browser| browser.content.matches(report, key))
        .collect();
    consumed.extend(external.iter().map(|browser| browser.input));
    if external.len() + usize::from(report.broker_browser.is_some()) > 1 {
        for browser in external {
            exclude(
                excluded,
                Source::Browser,
                browser.input,
                Rejection::DuplicateBrowserTraces,
                Some(browser.content.status),
            );
        }
        if report.broker_browser.is_some() {
            exclude(
                excluded,
                Source::Cli,
                cli.input,
                Rejection::DuplicateBrowserTraces,
                Some(report.status),
            );
        }
        return (Status::Blocked, None);
    }
    let (proof, source, input) = if let Some(browser) = external.first() {
        (&browser.content, Source::Browser, browser.input)
    } else if let Some(proof) = &report.broker_browser {
        (proof, Source::Cli, cli.input)
    } else {
        return match (report.status, report.stage.as_str()) {
            (Status::Pass, _) | (Status::Blocked, "broker_browser" | "browser_preflight") => {
                (Status::Blocked, None)
            }
            _ => (Status::NotRun, None),
        };
    };
    if proof.status != Status::Pass {
        return (proof.status, None);
    }
    let trace = (matches!(source, Source::Browser)
        || (report.status == Status::Pass && report.stage == "complete"))
        .then(|| proof.broker_trace(report, key, source, input))
        .flatten();
    if trace.is_none() {
        exclude(
            excluded,
            source,
            input,
            Rejection::IncompleteBrowserTrace,
            Some(proof.status),
        );
        return (Status::Blocked, None);
    }
    (Status::Pass, trace)
}

fn aggregate(
    candidate: Candidate,
    cli_reports: &[Report<CliReport>],
    browsers: &[Report<BrowserReport>],
    mut excluded: Vec<Exclusion>,
) -> Matrix {
    let selected: Vec<_> = cli_reports
        .iter()
        .filter_map(|cli| {
            match check_candidate(&cli.content, &candidate)
                .and_then(|()| CaseKey::from_cli(&cli.content, browsers))
            {
                Ok(key) => Some((key, cli)),
                Err(reason) => {
                    exclude(
                        &mut excluded,
                        Source::Cli,
                        cli.input,
                        reason,
                        Some(cli.content.status),
                    );
                    None
                }
            }
        })
        .collect();
    let transports: &[_] = match candidate.execution_path {
        ExecutionPath::Legacy => &[Transport::Batch, Transport::Tty],
        ExecutionPath::BrokerBatch => &[Transport::Batch],
    };
    let mut cases = Vec::with_capacity(4 * transports.len());
    let mut consumed = Vec::new();
    for client in [Client::Codex, Client::ClaudeCode] {
        for auth_mode in [AuthMode::Api, AuthMode::Subscription] {
            for &transport in transports {
                let key = CaseKey {
                    client,
                    auth_mode,
                    transport,
                };
                cases.push(aggregate_case(
                    key,
                    &selected,
                    cli_reports,
                    browsers,
                    &mut consumed,
                    &mut excluded,
                ));
            }
        }
    }
    for browser in browsers {
        if !consumed.contains(&browser.input) {
            exclude(
                &mut excluded,
                Source::Browser,
                browser.input,
                if browser.content.execution_path == candidate.execution_path {
                    Rejection::UnmatchedBrowserSessionOrAxes
                } else {
                    Rejection::OtherExecutionPath
                },
                Some(browser.content.status),
            );
        }
    }
    Matrix {
        version: 1,
        candidate,
        image_identity_source: if transports.len() == 1 {
            "broker_browser_observed_worker_image_id_matches_candidate"
        } else {
            "image_requested_for_attempt_worker_image_id_required_for_pass"
        },
        model_served: None,
        client_version: None,
        release_qualification: false,
        cases,
        excluded,
    }
}

fn aggregate_case(
    key: CaseKey,
    selected: &[(CaseKey, &Report<CliReport>)],
    cli_reports: &[Report<CliReport>],
    browsers: &[Report<BrowserReport>],
    consumed: &mut Vec<usize>,
    excluded: &mut Vec<Exclusion>,
) -> Case {
    let mut case = Case {
        key,
        model_requested: key.client.model(),
        status: Status::NotRun,
        cli_status: Status::NotRun,
        cli_input: None,
        cli_session_id: None,
        cli_reason: None,
        browser_status: Status::NotRun,
        browser: None,
        broker_browser: None,
    };
    let matches: Vec<_> = selected
        .iter()
        .filter_map(|(other, cli)| (*other == key).then_some(*cli))
        .collect();
    let Some(cli) = matches.first() else {
        return case;
    };
    let session = cli.content.session_id.filter(|id| !id.is_nil());
    // An excluded report can still contradict the identity of this session.
    if matches.len() > 1
        || session.is_some()
            && cli_reports
                .iter()
                .filter(|report| report.content.session_id == session)
                .take(2)
                .count()
                > 1
    {
        for cli in matches {
            exclude(
                excluded,
                Source::Cli,
                cli.input,
                Rejection::DuplicateCliCaseOrSession,
                Some(cli.content.status),
            );
        }
        case.cli_status = Status::Blocked;
        case.status = Status::Blocked;
        case.cli_reason = Some("duplicate_cli_case_or_session");
        return case;
    }
    case.cli_input = Some(cli.input);
    case.cli_session_id = session;
    match cli.content.execution_path {
        ExecutionPath::Legacy => {
            (case.cli_status, case.cli_reason) = cli.content.verdict(key.transport);
            (case.browser_status, case.browser) =
                browser_verdict(cli, key, browsers, consumed, excluded);
            // A replay can document the session after the original browser failure.
            if cli.content.status == Status::Fail && cli.content.stage == "browser" {
                case.browser_status = Status::Fail;
            }
        }
        ExecutionPath::BrokerBatch => {
            (case.cli_status, case.cli_reason) = cli.content.broker_verdict();
            if case.cli_status == Status::Pass
                && cli.content.status == Status::Fail
                && cli.content.stage == "broker_browser"
            {
                case.browser_status = Status::Fail;
            } else {
                (case.browser_status, case.broker_browser) =
                    broker_browser_verdict(cli, key, browsers, consumed, excluded);
            }
        }
    }
    let timeout = case
        .browser
        .as_ref()
        .is_some_and(|trace| trace.scope == BrowserScope::SessionTimeoutTrace);
    case.status = match (case.cli_status, case.browser_status) {
        _ if timeout => Status::Fail,
        (Status::Fail, _) | (_, Status::Fail) => Status::Fail,
        (Status::Blocked, _) | (_, Status::Blocked) => Status::Blocked,
        (Status::Pass, Status::Pass) => Status::Pass,
        _ => Status::NotRun,
    };
    case
}

fn write_matrix(
    mut output: impl Write,
    matrix: &Matrix,
    require_pass: bool,
) -> anyhow::Result<ExitCode> {
    serde_json::to_writer_pretty(&mut output, matrix)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(
        if require_pass && matrix.cases.iter().any(|case| case.status != Status::Pass) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
}

fn main() -> anyhow::Result<ExitCode> {
    let args = Args::parse();
    let candidate = args.candidate().unwrap_or_else(|message| {
        clap::Error::raw(clap::error::ErrorKind::ArgumentConflict, message).exit()
    });
    let mut excluded = Vec::new();
    let cli = read_reports(&args.cli_report, Source::Cli, &mut excluded);
    let browsers = read_reports(&args.browser_report, Source::Browser, &mut excluded);
    let matrix = aggregate(candidate, &cli, &browsers, excluded);
    write_matrix(io::stdout().lock(), &matrix, args.require_pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const SESSION: &str = "0de824ea-cfc9-4886-8363-6733185809c4";

    fn digest(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn candidate() -> Candidate {
        Candidate {
            host: "Darwin/arm64".into(),
            runtime: Runtime::Docker,
            execution_path: ExecutionPath::Legacy,
            cli_sha256: digest('a'),
            worker_image_sha256: format!("sha256:{}", digest('b')),
            rootful_exception_requested: Some(true),
        }
    }

    fn cli() -> Value {
        let candidate = candidate();
        json!({
            "status": "PASS", "stage": "complete", "host": candidate.host,
            "runtime": "docker", "cli_sha256": candidate.cli_sha256,
            "image_requested": candidate.worker_image_sha256, "worker_image_id": candidate.worker_image_sha256,
            "rootful_exception_requested": true, "client": "claude-code", "auth_mode": "api",
            "model_requested": "claude-haiku-5-5", "transport": "batch", "session_id": SESSION,
            "cli_exit_status": 0, "runtime_result": "terminal", "quota_released": true,
            "cleanup": "removed", "original_unchanged": true, "result_sha256": digest('c'),
            "audit_before": { "valid": true, "event_count": 3, "head_hash": digest('d') },
            "audit_after": { "valid": true, "event_count": 34, "head_hash": digest('e') },
            "diagnostic": "synthetic raw diagnostic must never be echoed"
        })
    }

    fn browser() -> Value {
        json!({
            "status": "PASS", "mode": "session_trace", "transport": "batch", "session": SESSION,
            "client": "claude_code", "authMode": "api", "requestedModel": "claude-haiku-5-5", "runtime": "docker",
            "decisionEventId": "00000000-0000-4000-8000-000000000001",
            "terminalEventId": "00000000-0000-4000-8000-000000000002",
            "exportEventId": "00000000-0000-4000-8000-000000000003",
            "fileEventId": "00000000-0000-4000-8000-000000000004",
            "fileSha256": digest('c'), "auditExportSha256": digest('f'), "count": 8,
            "browser": "157.0.8103.79"
        })
    }

    fn inputs<T: DeserializeOwned>(values: Vec<Value>) -> Vec<Report<T>> {
        values
            .into_iter()
            .enumerate()
            .map(|(index, value)| Report {
                input: index + 1,
                content: serde_json::from_value(value).unwrap(),
            })
            .collect()
    }

    fn matrix(cli: Vec<Value>, browser: Vec<Value>) -> Matrix {
        aggregate(candidate(), &inputs(cli), &inputs(browser), vec![])
    }

    const OPERATION: &str = "00000000-0000-4000-8000-000000000011";
    const EXPORT: &str = "00000000-0000-4000-8000-000000000012";
    const WORKER: &str = "00000000-0000-4000-8000-000000000013";
    const AUTHORITY: &str = "00000000-0000-4000-8000-000000000014";

    fn broker_cli() -> Value {
        let mut report = cli();
        report.as_object_mut().unwrap().remove("worker_image_id");
        report.as_object_mut().unwrap().extend(
            json!({
                "execution_path": "broker_batch", "status": "BLOCKED", "stage": "broker_browser",
                "profile": "fixture-api", "broker_operation_id": OPERATION,
                "broker_run_reply": "matched", "broker_cli_proof": "terminal_and_export_verified",
                "broker_export_command_id": EXPORT, "broker_export_request_id": EXPORT,
                "broker_export_manifest_sha256": digest('7'),
                "broker_terminal": {
                    "session_id": SESSION, "container_id": digest('9'), "manifest_digest": digest('8'),
                    "result_audit_seq": 6002, "scan_audit_seq": 6004, "cleanup_audit_seq": 6003,
                }
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        report
    }

    fn broker_browser() -> Value {
        json!({
            "status": "PASS", "mode": "broker_session_trace", "executionPath": "broker_batch",
            "transport": "batch", "session": SESSION, "operationId": OPERATION, "workerId": WORKER,
            "authorityId": AUTHORITY, "workerImageId": candidate().worker_image_sha256, "containerId": digest('9'),
            "client": "claude_code", "authMode": "api", "clientProfile": "fixture-api",
            "requestedModel": "claude-haiku-5-5", "runtime": "docker", "exportRequestId": EXPORT,
            "fileSha256": digest('c'), "scanManifestSha256": digest('8'), "exportManifestSha256": digest('7'),
            "decisionEventId": "00000000-0000-4000-8000-000000000001",
            "terminalEventId": "00000000-0000-4000-8000-000000000002",
            "scanEventId": "00000000-0000-4000-8000-000000000003",
            "cleanupEventId": "00000000-0000-4000-8000-000000000004",
            "exportEventId": "00000000-0000-4000-8000-000000000005",
            "decisionAuditSeq": 6000, "resultAuditSeq": 6002, "scanAuditSeq": 6004,
            "cleanupAuditSeq": 6003, "exportAuditSeq": 6010,
            "auditExportSha256": digest('f'), "count": 8, "browser": "157.0.8103.79"
        })
    }

    fn broker_matrix(cli: Vec<Value>, browser: Vec<Value>) -> Matrix {
        let mut candidate = candidate();
        candidate.execution_path = ExecutionPath::BrokerBatch;
        aggregate(candidate, &inputs(cli), &inputs(browser), vec![])
    }

    #[test]
    fn execution_paths_keep_legacy_defaults_and_reject_broker_tty() {
        let broker_session = json!(Uuid::from_u128(0x21));
        let mut batch = broker_cli();
        batch["session_id"] = broker_session.clone();
        batch["broker_terminal"]["session_id"] = broker_session.clone();
        let mut broker_trace = broker_browser();
        broker_trace["session"] = broker_session;
        let legacy = matrix(vec![cli(), batch.clone()], vec![browser(), broker_trace]);
        assert_eq!(legacy.cases.len(), 8);
        assert_eq!(claude_api(&legacy, Transport::Batch).status, Status::Pass);
        assert!(
            serde_json::to_value(&legacy).unwrap()["candidate"]
                .get("execution_path")
                .is_none()
        );
        assert!(
            legacy
                .excluded
                .iter()
                .all(|entry| entry.reason == Rejection::OtherExecutionPath)
        );

        let mut tty = broker_cli();
        tty["transport"] = json!("tty");
        let tty_session = json!(Uuid::from_u128(0x22));
        tty["session_id"] = tty_session.clone();
        tty["broker_terminal"]["session_id"] = tty_session;
        let broker = broker_matrix(vec![cli(), tty, batch], vec![browser()]);
        assert_eq!(broker.cases.len(), 4);
        assert!(
            broker
                .cases
                .iter()
                .all(|case| case.key.transport == Transport::Batch)
        );
        assert_eq!(
            claude_api(&broker, Transport::Batch).status,
            Status::Blocked
        );
        assert!(
            broker
                .excluded
                .iter()
                .any(|entry| entry.reason == Rejection::IncompatibleExecutionPathTransport)
        );
        assert_eq!(
            serde_json::to_value(&broker).unwrap()["candidate"]["execution_path"],
            "broker_batch"
        );
    }

    #[test]
    fn broker_candidate_rejects_kubernetes_without_changing_legacy() {
        let binary = digest('a');
        let image = digest('b');
        let args = [
            "provider-matrix",
            "--host",
            "Darwin/arm64",
            "--runtime",
            "kubernetes",
            "--cli-sha256",
            &binary,
            "--worker-image-sha256",
            &image,
        ];
        let legacy = Args::try_parse_from(args).unwrap().candidate().unwrap();
        assert_eq!(legacy.execution_path, ExecutionPath::Legacy);
        assert_eq!(legacy.runtime, Runtime::Kubernetes);
        let broker =
            Args::try_parse_from(args.into_iter().chain(["--execution-path", "broker-batch"]))
                .unwrap();
        assert_eq!(
            broker.candidate().unwrap_err(),
            "--execution-path broker-batch supports Docker and Podman only"
        );
    }

    #[test]
    fn broker_correlates_embedded_or_standalone_trace_without_using_event_count_as_sequence() {
        let selected = broker_matrix(vec![broker_cli()], vec![broker_browser()]);
        let row = claude_api(&selected, Transport::Batch);
        assert_eq!(
            (row.status, row.cli_status, row.browser_status),
            (Status::Pass, Status::Pass, Status::Pass)
        );
        assert!(row.browser.is_none());
        assert_eq!(row.broker_browser.as_ref().unwrap().export_audit_seq, 6010);
        assert!(selected.model_served.is_none());

        let mut inline = broker_cli();
        inline["status"] = json!("PASS");
        inline["stage"] = json!("complete");
        inline["worker_id"] = json!(WORKER);
        inline["authority_id"] = json!(AUTHORITY);
        inline["worker_image_id"] = json!(candidate().worker_image_sha256);
        inline["broker_browser"] = broker_browser();
        let result = broker_matrix(vec![inline], vec![]);
        assert_eq!(claude_api(&result, Transport::Batch).status, Status::Pass);
    }

    #[test]
    fn broker_requires_the_same_operation_worker_artifacts_and_audit_sequences() {
        for (field, invalid) in [
            ("operationId", json!(Uuid::nil())),
            ("workerId", json!(Uuid::nil())),
            ("authorityId", json!(Uuid::nil())),
            ("workerImageId", json!(format!("sha256:{}", digest('5')))),
            ("containerId", json!(digest('5'))),
            ("clientProfile", json!("other")),
            ("authMode", json!("subscription")),
            ("requestedModel", json!("haiku")),
            ("runtime", json!("podman")),
            ("scanManifestSha256", json!(digest('5'))),
            ("exportManifestSha256", json!(digest('5'))),
            ("fileSha256", json!(digest('5'))),
            ("exportRequestId", json!(OPERATION)),
            ("decisionAuditSeq", json!(6011)),
            ("resultAuditSeq", json!(6999)),
            ("scanAuditSeq", json!(6002)),
            ("cleanupAuditSeq", Value::Null),
            ("exportAuditSeq", json!(0)),
            ("exportAuditSeq", json!(MAX_SAFE_JS_INTEGER + 1)),
            ("count", json!(4)),
        ] {
            let mut proof = broker_browser();
            proof[field] = invalid;
            let out = broker_matrix(vec![broker_cli()], vec![proof]);
            let row = claude_api(&out, Transport::Batch);
            assert_eq!(row.status, Status::Blocked, "{field}");
            assert!(row.broker_browser.is_none(), "{field}");
        }

        for (field, invalid) in [
            ("worker_id", json!(EXPORT)),
            ("authority_id", json!(EXPORT)),
            ("broker_export_request_id", json!(OPERATION)),
            ("broker_run_reply", json!("unavailable")),
            ("cli_exit_status", Value::Null),
        ] {
            let mut cli = broker_cli();
            cli[field] = invalid;
            let out = broker_matrix(vec![cli], vec![broker_browser()]);
            assert_eq!(
                claude_api(&out, Transport::Batch).status,
                Status::Blocked,
                "{field}"
            );
        }
    }

    #[test]
    fn broker_needs_browser_scope_and_preserves_a_failed_original_run() {
        let without_browser = broker_matrix(vec![broker_cli()], vec![]);
        let row = claude_api(&without_browser, Transport::Batch);
        assert_eq!(
            (row.status, row.cli_status, row.browser_status),
            (Status::Blocked, Status::Pass, Status::Blocked)
        );

        let mut failed = broker_cli();
        failed["status"] = json!("FAIL");
        failed["broker_run_reply"] = json!("unavailable");
        failed["cli_exit_status"] = Value::Null;
        let still_failed = broker_matrix(vec![failed], vec![broker_browser()]);
        let row = claude_api(&still_failed, Transport::Batch);
        assert_eq!((row.status, row.cli_status), (Status::Fail, Status::Fail));

        let mut browser_failed = broker_cli();
        browser_failed["status"] = json!("FAIL");
        let still_failed = broker_matrix(vec![browser_failed], vec![broker_browser()]);
        let row = claude_api(&still_failed, Transport::Batch);
        assert_eq!(
            (row.status, row.cli_status, row.browser_status),
            (Status::Fail, Status::Pass, Status::Fail)
        );
    }

    #[test]
    fn require_pass_emits_the_same_json_and_rejects_each_non_pass_status() {
        let mut matrix = matrix(vec![], vec![]);
        for case in &mut matrix.cases {
            case.status = Status::Pass;
        }

        for (status, strict_exit) in [
            (Status::Pass, ExitCode::SUCCESS),
            (Status::Fail, ExitCode::FAILURE),
            (Status::Blocked, ExitCode::FAILURE),
            (Status::NotRun, ExitCode::FAILURE),
        ] {
            matrix.cases[0].status = status;
            let mut consultation = Vec::new();
            let mut strict = Vec::new();
            assert_eq!(
                write_matrix(&mut consultation, &matrix, false).unwrap(),
                ExitCode::SUCCESS
            );
            assert_eq!(
                write_matrix(&mut strict, &matrix, true).unwrap(),
                strict_exit
            );
            assert_eq!(strict, consultation);
            let report: Value = serde_json::from_slice(&strict).unwrap();
            assert_eq!(report["cases"][0]["status"], json!(status));
            assert_eq!(report["release_qualification"], false);
        }
    }

    fn claude_api(matrix: &Matrix, transport: Transport) -> &Case {
        matrix
            .cases
            .iter()
            .find(|case| {
                case.key
                    == CaseKey {
                        client: Client::ClaudeCode,
                        auth_mode: AuthMode::Api,
                        transport,
                    }
            })
            .unwrap()
    }

    #[test]
    fn legacy_batch_uses_matching_explicit_browser_axes_and_file() {
        let mut legacy = cli();
        legacy.as_object_mut().unwrap().remove("transport");
        let good = matrix(vec![legacy.clone()], vec![browser()]);
        let case = claude_api(&good, Transport::Batch);
        assert_eq!(good.cases.len(), 8);
        assert_eq!(
            (case.status, case.cli_status, case.browser_status),
            (Status::Pass, Status::Pass, Status::Pass)
        );
        let trace = case.browser.as_ref().unwrap();
        assert_eq!(trace.session_id.to_string(), SESSION);
        assert_eq!(trace.key, case.key);
        assert!(trace.bundle_sha256.is_none() && good.model_served.is_none());
        let encoded = serde_json::to_string(&good).unwrap();
        assert!(!encoded.contains("raw diagnostic"));

        for (field, invalid) in [
            ("transport", json!(null)),
            ("session", json!(Uuid::nil())),
            ("client", json!("codex")),
            ("authMode", json!("subscription")),
            ("requestedModel", json!("haiku")),
            ("runtime", json!("podman")),
            ("fileSha256", json!(digest('b'))),
        ] {
            let mut mismatch = browser();
            mismatch[field] = invalid;
            let out = matrix(vec![legacy.clone()], vec![mismatch]);
            assert_eq!(
                claude_api(&out, Transport::Batch).status,
                Status::NotRun,
                "{field}"
            );
            assert_eq!(out.excluded[0].reason, Rejection::UnknownTransport);
        }
    }

    #[test]
    fn legacy_browser_failure_cannot_be_replaced_by_a_passing_replay() {
        let mut report = cli();
        report["stage"] = json!("browser");
        report["status"] = json!("FAIL");
        let failed = matrix(vec![report.clone()], vec![browser()]);
        let case = claude_api(&failed, Transport::Batch);
        assert_eq!(
            (case.status, case.cli_status, case.browser_status),
            (Status::Fail, Status::Pass, Status::Fail)
        );
        assert_eq!(case.browser.as_ref().unwrap().source, Source::Browser);

        report["status"] = json!("BLOCKED");
        let blocked = matrix(vec![report.clone()], vec![]);
        assert_eq!(
            claude_api(&blocked, Transport::Batch).status,
            Status::Blocked
        );
        let recovered = matrix(vec![report], vec![browser()]);
        let case = claude_api(&recovered, Transport::Batch);
        assert_eq!(
            (case.status, case.cli_status, case.browser_status),
            (Status::Pass, Status::Pass, Status::Pass)
        );
    }

    #[test]
    fn candidate_filters_artifact_and_platform_axes_and_keeps_unscoped_preflight_out() {
        for (field, invalid) in [
            ("host", json!("Linux/x86_64")),
            ("runtime", json!("podman")),
            ("cli_sha256", json!(digest('f'))),
            ("image_requested", json!(format!("sha256:{}", digest('f')))),
            ("rootful_exception_requested", json!(false)),
        ] {
            let mut other = cli();
            other[field] = invalid;
            let out = matrix(vec![other], vec![]);
            assert_eq!(out.excluded[0].reason, Rejection::OtherCandidate, "{field}");
            assert!(out.cases.iter().all(|case| case.status == Status::NotRun));
        }
        let mut tagged = cli();
        tagged["image_requested"] = json!("worker:mutable");
        let blocked = json!({ "status": "BLOCKED", "stage": "provider_profile", "client": "claude-code",
            "auth_mode": "subscription", "runtime": "docker", "model_requested": "claude-haiku-5-5" });
        let out = matrix(vec![tagged, blocked], vec![browser()]);
        assert_eq!(out.excluded[0].reason, Rejection::MissingCandidateIdentity);
        assert_eq!(out.excluded[1].reported_status, Some(Status::Blocked));
        assert_eq!(out.excluded[1].reason, Rejection::MissingCandidateIdentity);
        assert!(out.cases.iter().all(|case| case.status == Status::NotRun));
    }

    #[test]
    fn same_session_in_another_candidate_blocks_the_case() {
        for (field, invalid) in [
            ("cli_sha256", json!(digest('f'))),
            ("image_requested", json!(format!("sha256:{}", digest('f')))),
        ] {
            let mut other = cli();
            other[field] = invalid;
            other["worker_image_id"] = other["image_requested"].clone();
            let out = matrix(vec![cli(), other.clone()], vec![browser()]);
            let row = claude_api(&out, Transport::Batch);
            assert_eq!(
                (row.status, row.cli_status),
                (Status::Blocked, Status::Blocked),
                "{field}"
            );
            assert_eq!(
                row.cli_reason,
                Some("duplicate_cli_case_or_session"),
                "{field}"
            );
            assert!(row.browser.is_none(), "{field}");
            assert!(
                out.excluded
                    .iter()
                    .any(|entry| entry.input == 2 && entry.reason == Rejection::OtherCandidate),
                "{field}"
            );

            other["session_id"] = json!(Uuid::from_u128(10));
            let out = matrix(vec![cli(), other], vec![browser()]);
            assert_eq!(
                claude_api(&out, Transport::Batch).status,
                Status::Pass,
                "{field}"
            );
            assert_eq!(out.excluded.len(), 1, "{field}");
            assert_eq!(out.excluded[0].reason, Rejection::OtherCandidate, "{field}");
        }
    }

    #[test]
    fn pass_requires_observed_image_and_full_cli_evidence() {
        for field in [
            "worker_image_id",
            "runtime_result",
            "quota_released",
            "cleanup",
            "original_unchanged",
            "result_sha256",
            "audit_after",
            "audit_before",
            "cli_exit_status",
        ] {
            let mut missing = cli();
            missing.as_object_mut().unwrap().remove(field);
            let out = matrix(vec![missing], vec![browser()]);
            let case = claude_api(&out, Transport::Batch);
            assert_eq!(case.cli_status, Status::Fail, "{field}");
            assert_eq!(case.status, Status::Fail, "{field}");
        }
    }

    #[test]
    fn tty_needs_two_turns_and_a_passed_timeout_view_leaves_the_run_failed() {
        let mut tty_cli = cli();
        tty_cli["transport"] = json!("tty");
        tty_cli["new_checkpoint_count"] = json!(1);
        tty_cli["result_sha256"] = Value::Null;
        tty_cli["tty"] =
            json!({"stage":"complete","receipts":2,"termios_restored":true,"cli_exit_status":0});
        let mut tty_browser = browser();
        tty_browser["transport"] = json!("tty");
        tty_browser["fileEventId"] = Value::Null;
        tty_browser["fileSha256"] = Value::Null;
        let out = matrix(vec![tty_cli.clone()], vec![tty_browser.clone()]);
        assert_eq!(claude_api(&out, Transport::Tty).status, Status::Pass);

        let terminal = json!({
            "exit_code": 0, "oom_killed": false, "cleanup_removed": true, "timed_out": false,
            "transport_stop_reason": null, "control_deadline": null,
        });
        let mut observed = tty_cli.clone();
        observed["tty_terminal_result"] = terminal;
        let out = matrix(vec![observed.clone()], vec![tty_browser.clone()]);
        assert_eq!(claude_api(&out, Transport::Tty).status, Status::Pass);
        for (field, value) in [
            ("exit_code", json!(137)),
            ("oom_killed", json!(true)),
            ("cleanup_removed", json!(false)),
            ("timed_out", json!(true)),
            ("transport_stop_reason", json!("terminal transport closed")),
            (
                "control_deadline",
                json!({"deadline":"original_execution_deadline","stop":"already_stopped"}),
            ),
            (
                "control_deadline",
                json!({"deadline":"observation_window","stop":"unconfirmed"}),
            ),
        ] {
            let mut conflicting = observed.clone();
            conflicting["tty_terminal_result"][field] = value;
            let out = matrix(vec![conflicting], vec![tty_browser.clone()]);
            let case = claude_api(&out, Transport::Tty);
            assert_eq!(case.cli_status, Status::Fail, "{field}");
            assert_eq!(case.status, Status::Fail, "{field}");
        }

        tty_cli["tty"]["termios_restored"] = json!(false);
        let out = matrix(vec![tty_cli.clone()], vec![tty_browser.clone()]);
        assert_eq!(claude_api(&out, Transport::Tty).cli_status, Status::Fail);
        tty_cli["tty"]["receipts"] = json!(1);
        tty_cli["status"] = json!("FAIL");
        tty_cli["stage"] = json!("interactive_conversation");
        tty_browser["mode"] = json!("session_timeout_trace");
        tty_browser["terminalKind"] = json!("command.failed");
        tty_browser["terminalExitCode"] = json!(137);
        tty_browser["workerOutcome"] = json!("timeout");
        tty_browser["timeoutLabelsVerified"] = json!(true);
        tty_browser["exportEventId"] = Value::Null;
        let out = matrix(vec![tty_cli], vec![tty_browser]);
        let case = claude_api(&out, Transport::Tty);
        assert_eq!(case.status, Status::Fail);
        assert_eq!(case.browser_status, Status::Pass);
        assert_eq!(
            case.browser.as_ref().unwrap().scope,
            BrowserScope::SessionTimeoutTrace
        );
        assert_eq!(
            case.browser.as_ref().unwrap().timeout_labels_verified,
            Some(true)
        );
    }

    #[test]
    fn candidate_worker_and_export_manifest_are_bound_when_present() {
        let mut cli = cli();
        let mut browser = browser();
        let worker = "2f9bf0c5-7279-4cc3-bd2e-dbae88d0d5d1";
        let manifest = digest('a');
        cli["worker_id"] = json!(worker);
        browser["workerId"] = json!(worker);
        browser["exportManifestSha256"] = json!(manifest);
        let good = matrix(vec![cli.clone()], vec![browser.clone()]);
        let case = claude_api(&good, Transport::Batch);
        assert_eq!(case.status, Status::Pass);
        let trace = case.browser.as_ref().unwrap();
        assert_eq!(trace.worker_id, Some(Uuid::parse_str(worker).unwrap()));
        assert_eq!(
            trace.export_manifest_sha256.as_deref(),
            Some(manifest.as_str())
        );

        for (field, invalid) in [
            ("workerId", json!("80fff301-6137-4cf3-9531-1c514e74304c")),
            ("workerId", Value::Null),
            ("exportManifestSha256", Value::Null),
            ("exportManifestSha256", json!("incomplete")),
        ] {
            let mut proof = browser.clone();
            proof[field] = invalid;
            let mismatch = matrix(vec![cli.clone()], vec![proof]);
            assert_eq!(claude_api(&mismatch, Transport::Batch).status, Status::Fail);
            assert_eq!(
                mismatch.excluded[0].reason,
                Rejection::IncompleteBrowserTrace
            );
        }
    }

    #[test]
    fn embedded_trace_correlates_with_its_cli_and_explicit_replays_cannot_be_merged() {
        let mut embedded = cli();
        let raw = browser();
        embedded["browser"] = json!({
            "session_id": SESSION, "file_sha256": raw["fileSha256"], "fileEventId": raw["fileEventId"],
            "exportEventId": raw["exportEventId"], "decisionEventId": raw["decisionEventId"],
            "terminalEventId": raw["terminalEventId"], "audit_export_sha256": raw["auditExportSha256"],
            "audit_export_event_count": raw["count"], "chromium_version": raw["browser"]
        });
        let ok = matrix(vec![embedded.clone()], vec![]);
        assert_eq!(claude_api(&ok, Transport::Batch).status, Status::Pass);
        let duplicate = matrix(vec![embedded.clone()], vec![browser()]);
        let case = claude_api(&duplicate, Transport::Batch);
        assert_eq!(
            (case.cli_status, case.browser_status, case.status),
            (Status::Pass, Status::Blocked, Status::Blocked)
        );
        assert!(
            duplicate
                .excluded
                .iter()
                .all(|item| item.reason == Rejection::DuplicateBrowserTraces)
        );

        embedded["browser"]["session_id"] = json!(Uuid::nil());
        let bad = matrix(vec![embedded], vec![]);
        assert_eq!(
            claude_api(&bad, Transport::Batch).browser_status,
            Status::Fail
        );
        let duplicates = matrix(vec![cli(), cli()], vec![browser()]);
        assert_eq!(
            claude_api(&duplicates, Transport::Batch).status,
            Status::Blocked
        );
        assert_eq!(
            duplicates.excluded[0].reason,
            Rejection::DuplicateCliCaseOrSession
        );
    }
}
