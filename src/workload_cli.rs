//! Explicit operator provisioning through the workload binding and broker APIs.
//! Private preparations survive the process; receipts describe historical recording.
use clap::{Args, Subcommand};
use serde::Serialize;
use std::{io::Write, path::PathBuf};

pub(crate) mod broker_binding;
mod http;
mod manifest;
mod storage;

type Result<T> = std::result::Result<T, WorkloadCliError>;

#[derive(Args)]
pub struct WorkloadArgs {
    #[command(subcommand)]
    command: WorkloadCommand,
}

#[derive(Subcommand)]
enum WorkloadCommand {
    /// Prepare an immutable request locally; credential issue/rotation also creates a private key.
    Prepare {
        /// Private closed manifest, or '-' for bounded public standard input.
        #[arg(long)]
        input: PathBuf,
        /// Private gateway, expected tenant/namespace and credential references.
        #[arg(long)]
        control_file: PathBuf,
        /// New private directory below an existing private control parent.
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Submit the exact retained preparation once with the current control proof.
    Apply(Connection),
    /// Recover the historical receipt by submitting the exact retained operation.
    Resume(Connection),
    /// Inspect retained local evidence; current remote state remains unobserved.
    Status {
        #[arg(long)]
        operation_dir: PathBuf,
    },
}

#[derive(Args)]
struct Connection {
    #[arg(long)]
    operation_dir: PathBuf,
    /// Current private runner/CA references; retained scope expectations must match.
    #[arg(long)]
    control_file: PathBuf,
}

/// Closed categories never retain remote text, credentials or parser/IO chains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkloadCliError {
    InvalidInput,
    InputDeadline,
    InvalidControl,
    UnsafeState,
    IncompletePreparation,
    Busy,
    ScopeMismatch,
    Transport,
    Protocol,
    Authentication,
    Forbidden,
    Conflict,
    Capacity,
    Clock,
    Unavailable,
    Output,
}

impl std::fmt::Display for WorkloadCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use WorkloadCliError::*;
        f.write_str(match self {
            InvalidInput => "workload input is invalid, unsupported or exceeds 16 KiB; check the closed manifest",
            InputDeadline => "workload standard input exceeded its 5 second deadline",
            InvalidControl => "workload control configuration or private runner/CA file is invalid or unsafe",
            UnsafeState => "workload private state is unsafe, changed, conflicting or exceeds its bound; preserve all evidence",
            IncompletePreparation => "workload preparation is incomplete; preserve it without overwriting or regenerating its key",
            Busy => "workload preparation is already in use; retry after its owner exits",
            ScopeMismatch => "workload authenticated control scope differs from the prepared gateway, tenant or namespace; no POST was authorized",
            Transport => "workload connection, TLS, timeout or response delivery failed; preserve state and explicitly resume the same operation",
            Protocol => "workload response is incompatible or unbounded; outcome is unresolved, preserve state and resume only the same operation",
            Authentication => "workload runner authentication failed; select a current private runner file for the same prepared scope",
            Forbidden => "workload operation is forbidden; preserve preparation and correct authority through trusted control",
            Conflict => "workload operation conflicts with durable state; preserve the exact request and compare its CAS and receipt",
            Capacity => "workload control admission or audit capacity refused the operation; contact the operator, then resume only the same preparation",
            Clock => "workload control clock is uncertain; preserve state and contact the operator before resuming",
            Unavailable => "workload control, identity or mandatory storage is unavailable; preserve state before explicitly resuming",
            Output => "workload output is unavailable or would expose protected data; inspect the retained preparation with status",
        })
    }
}
impl std::error::Error for WorkloadCliError {}

pub async fn run(args: WorkloadArgs) -> Result<()> {
    match args.command {
        WorkloadCommand::Prepare {
            input,
            control_file,
            output_dir,
        } => {
            let control = manifest::Control::load(&control_file)?;
            let material = http::Material::load(&control)?;
            let manifest = manifest::Manifest::read(&input)?;
            let operation =
                storage::Operation::prepare(manifest, &control, &output_dir, material.token())?;
            operation.report("prepared", None, &[material.token()])
        }
        WorkloadCommand::Apply(args) | WorkloadCommand::Resume(args) => apply(args).await,
        WorkloadCommand::Status { operation_dir } => {
            let operation = storage::Operation::open(&operation_dir)?;
            let receipt = operation.receipt()?;
            operation.report(
                if receipt.is_some() {
                    "operation_recorded"
                } else if !operation.has_scope()? {
                    "prepared"
                } else {
                    "remote_outcome_unresolved"
                },
                receipt.as_ref(),
                &[],
            )
        }
    }
}

async fn apply(args: Connection) -> Result<()> {
    let operation = storage::Operation::open(&args.operation_dir)?;
    let control = manifest::Control::load(&args.control_file)?;
    operation.require_control(&control)?;
    let material = http::Material::load(&control)?;
    operation.protect(&operation.recovery, material.token())?;
    // Validate every local authority/key/scope even if a historical receipt exists.
    let previous = operation.receipt()?;
    let client = http::Client::new(&control, &material)?;
    operation.revalidate()?;
    material.revalidate()?;
    control.revalidate()?;
    let protocol = client.protocol().await?;
    operation.protect(&protocol, material.token())?;
    operation.confirm_scope(&protocol)?;
    operation.revalidate()?;
    material.revalidate()?;
    control.revalidate()?;
    let receipt = client.post(&operation.recovery).await?;
    operation.protect(&receipt, material.token())?;
    operation.revalidate()?;
    material.revalidate()?;
    control.revalidate()?;
    if previous.as_ref().is_some_and(|old| old != &receipt) {
        return Err(WorkloadCliError::Conflict);
    }
    operation.save_receipt(&receipt)?;
    operation.report("operation_recorded", Some(&receipt), &[material.token()])
}

fn print_json(value: &impl Serialize, secrets: &[&str]) -> Result<()> {
    let value = serde_json::to_value(value).map_err(|_| WorkloadCliError::Output)?;
    if contains_literal(&value, secrets) {
        return Err(WorkloadCliError::Output);
    }
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|_| WorkloadCliError::Output)?;
    bytes.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| WorkloadCliError::Output)
}

/// Compare decoded literals before JSON escaping can hide a recoverable key.
/// Field names and values follow the same literal rule, with no semantic guess.
fn contains_literal(value: &serde_json::Value, secrets: &[&str]) -> bool {
    let protected = |text: &str| {
        secrets
            .iter()
            .any(|secret| !secret.is_empty() && text.contains(secret))
    };
    match value {
        serde_json::Value::String(text) => protected(text),
        serde_json::Value::Array(values) => {
            values.iter().any(|value| contains_literal(value, secrets))
        }
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(name, value)| protected(name) || contains_literal(value, secrets)),
        _ => false,
    }
}
