//! Explicit read-only Kubernetes diagnostics for the trusted control CLI.
//! No ambient configuration, credential discovery or workload admission.
use crate::{
    control_fs::PrivateFile,
    redaction::Redactor,
    runtime::kubernetes::{
        KubernetesAdmissionGapV1, KubernetesAuthorityV1, KubernetesCredentials,
        KubernetesDiagnosticsV1, KubernetesError, KubernetesRuntime,
    },
};
use clap::{ArgGroup, Args, Subcommand};
use serde::Serialize;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const MAX_AUTHORITY_BYTES: u64 = 16 * 1024;
const MAX_CERTIFICATE_BYTES: u64 = 64 * 1024;
const MAX_KEY_BYTES: u64 = 16 * 1024;
const MAX_TOKEN_BYTES: u64 = 8192;

#[derive(Args)]
pub struct KubernetesArgs {
    #[command(subcommand)]
    command: KubernetesCommand,
}

#[derive(Subcommand)]
enum KubernetesCommand {
    /// Inspect an explicit Kubernetes authority over HTTPS; never admit work.
    Doctor(DoctorArgs),
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
}
impl std::fmt::Display for KubernetesCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAuthority => f.write_str("Kubernetes authority configuration is invalid or unavailable; use an unchanged private authority file"),
            Self::InvalidCredentials => f.write_str("Kubernetes private credentials or trust are invalid or unavailable; verify the explicit private files"),
            Self::Inspection(error) => std::fmt::Display::fmt(error, f),
            Self::ProtectedOutput => f.write_str("Kubernetes diagnostic output contains protected credential material; inspect the source privately"),
            Self::OutputUnavailable => f.write_str("Kubernetes diagnostic output is unavailable"),
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

pub async fn execute(args: KubernetesArgs) -> Result<(), KubernetesCliError> {
    match args.command {
        KubernetesCommand::Doctor(args) => doctor(args).await,
    }
}

async fn doctor(args: DoctorArgs) -> Result<(), KubernetesCliError> {
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
    let redactor = Redactor::new(protected);
    source
        .revalidate()
        .map_err(|_| KubernetesCliError::InvalidAuthority)?;
    let runtime = KubernetesRuntime::connect(authority, credentials)
        .map_err(KubernetesCliError::Inspection)?;
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
