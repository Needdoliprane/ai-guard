//! Command line of `ai-guard-audit-checkpoint`.
//!
//! Every invocation prints exactly one JSON document on standard output.
//! Exit status 0 is success, 2 is an integrity, trust or scope refusal and 1
//! is an operational error, including an invalid command line. No option
//! disables a check, and nothing is sent anywhere.

use super::{
    COMMAND_FORMAT, ERROR_FORMAT,
    anchor::{
        AdvanceStatus, AnchorAdvanceRequest, AnchorInitRequest, RotationApprovalRequest,
        advance_anchor, approve_rotation, init_anchor,
    },
    bundle::{ExportRequest, export_bundle},
    create::{CreateRequest, RetiringKey, RotateRequest, create_checkpoint, rotate_key},
    diagnose::{DiagnoseRequest, diagnose},
    keys::{init_key, show_key},
    report::{Code, ErrorEntry, Failure},
    verify::{RecordSource, VerifyRequest, verify},
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use serde::Serialize;
use serde_json::{Value, json};
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "ai-guard-audit-checkpoint",
    version,
    about = "Create, sign and verify offline checkpoints of a tenant audit chain",
    long_about = "Create, sign and verify offline checkpoints of a tenant audit chain.\n\nEvery command prints one JSON document on standard output. Exit status: 0 success, 2 integrity/trust/scope refusal, 1 operational error. A signature is trusted only through a key pinned in a trust anchor that is held outside the controlled storage."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create or show the Ed25519 signing key of the control
    #[command(subcommand)]
    Key(KeyCommand),
    /// Manage the trust anchor held outside the controlled storage
    #[command(subcommand)]
    Anchor(AnchorCommand),
    /// Sign the current head of an audit database and append the record
    Create(CreateArgs),
    /// Record the hand-over of the log from the current key to a successor key
    RotateKey(RotateKeyArgs),
    /// Verify checkpoint records, and optionally a database, against a trust anchor
    Verify(VerifyArgs),
    /// Explain the state of a database, log, key and anchor without any trust decision
    Diagnose(DiagnoseArgs),
    /// Export contiguous records as a public proof bundle that carries no key
    Export(ExportArgs),
}

/// The records to read: the private checkpoint directory, or a proof bundle.
#[derive(Debug, Args)]
struct SourceArgs {
    /// Private directory of the checkpoint records
    #[arg(long, required_unless_present = "bundle", conflicts_with = "bundle")]
    checkpoint_dir: Option<PathBuf>,
    /// Public proof bundle produced by export
    #[arg(long)]
    bundle: Option<PathBuf>,
}

impl SourceArgs {
    fn source(&self) -> Result<RecordSource<'_>, Failure> {
        match (&self.checkpoint_dir, &self.bundle) {
            (Some(directory), None) => Ok(RecordSource::Directory(directory)),
            (None, Some(bundle)) => Ok(RecordSource::Bundle(bundle)),
            _ => Err(Failure::new(
                Code::InvalidArgument,
                "name exactly one of --checkpoint-dir and --bundle",
            )),
        }
    }
}

#[derive(Debug, Args)]
struct DiagnoseArgs {
    /// Audit SQLite database of the tenant; opened read-only
    #[arg(long, requires = "tenant_id")]
    data: Option<PathBuf>,
    #[arg(long, requires = "data")]
    tenant_id: Option<String>,
    #[arg(long)]
    checkpoint_dir: Option<PathBuf>,
    #[arg(long)]
    key_dir: Option<PathBuf>,
    #[arg(long)]
    anchor: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[arg(long)]
    checkpoint_dir: PathBuf,
    /// New file receiving the bundle
    #[arg(long)]
    out: PathBuf,
    /// First log index to export; the lowest available by default
    #[arg(long)]
    from_log_index: Option<u64>,
    /// Last log index to export; the head of the log by default
    #[arg(long)]
    through_log_index: Option<u64>,
}

#[derive(Debug, Subcommand)]
enum KeyCommand {
    /// Create a signing key in a new private directory
    Init {
        /// New directory; it is created with mode 0700 and never reused
        #[arg(long)]
        key_dir: PathBuf,
    },
    /// Show the public fingerprint and optionally export the public key document
    Show {
        #[arg(long)]
        key_dir: PathBuf,
        /// New file receiving the public key document
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AnchorCommand {
    /// Approve one public key explicitly for one tenant and control namespace
    Init {
        /// Public key document handed over by the operator
        #[arg(long)]
        public_key: PathBuf,
        /// Fingerprint received through a separate channel (64 lowercase hex)
        #[arg(long)]
        expect_key_id: String,
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        namespace_uuid: String,
        /// New file, outside the storage controlled by the signer
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify, then record the verified head as the accepted reference in a new revision
    Advance {
        /// Current trust anchor revision; it is kept unchanged
        #[arg(long)]
        anchor: PathBuf,
        #[command(flatten)]
        source: SourceArgs,
        /// Audit SQLite database to compare before accepting the reference; without it
        /// the positions pinned so far are retained in the new revision. With it, the
        /// retained positions are confirmed and dropped, even when no record is new
        #[arg(long)]
        data: Option<PathBuf>,
        /// New file receiving the next trust anchor revision
        #[arg(long)]
        out: PathBuf,
    },
    /// Approve the successor key of the first rotation recorded after the reference
    ApproveRotation {
        /// Current trust anchor revision; it is kept unchanged
        #[arg(long)]
        anchor: PathBuf,
        #[command(flatten)]
        source: SourceArgs,
        /// Public key document of the successor key
        #[arg(long)]
        public_key: PathBuf,
        /// Successor fingerprint received through a separate channel (64 lowercase hex)
        #[arg(long)]
        expect_key_id: String,
        /// Accept a rotation that the retiring key did not endorse (lost or compromised key)
        #[arg(long)]
        unendorsed_recovery: bool,
        /// Audit SQLite database to compare before the reference moves to the rotation
        #[arg(long)]
        data: Option<PathBuf>,
        /// New file receiving the next trust anchor revision
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Args)]
struct RotateKeyArgs {
    #[arg(long)]
    checkpoint_dir: PathBuf,
    /// Directory of the successor key, created beforehand with key init
    #[arg(long)]
    new_key_dir: PathBuf,
    /// Directory of the retiring key, which endorses its successor
    #[arg(
        long,
        required_unless_present = "unendorsed_recovery",
        conflicts_with_all = ["unendorsed_recovery", "retiring_key_id"]
    )]
    key_dir: Option<PathBuf>,
    /// The retiring key is lost or compromised: record a rotation without endorsement
    #[arg(long, requires = "retiring_key_id")]
    unendorsed_recovery: bool,
    /// Fingerprint of the retiring key (64 lowercase hex), with --unendorsed-recovery
    #[arg(long, requires = "unendorsed_recovery")]
    retiring_key_id: Option<String>,
}

#[derive(Debug, Args)]
struct CreateArgs {
    /// Audit SQLite database of the tenant; opened read-only
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    key_dir: PathBuf,
    /// Private directory of the checkpoint records, outside the database directory
    #[arg(long)]
    checkpoint_dir: PathBuf,
    /// Start a checkpoint log: required, and only accepted, while the directory
    /// holds no complete record. The first record is a baseline.
    #[arg(long)]
    new_log: bool,
    /// Last record of a previous, full checkpoint directory; starts a new
    /// directory linked to it
    #[arg(long)]
    previous_record: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Trust anchor held outside the controlled storage
    #[arg(long)]
    anchor: PathBuf,
    #[command(flatten)]
    source: SourceArgs,
    /// Audit SQLite database to compare with every trusted signed position
    #[arg(long)]
    data: Option<PathBuf>,
    /// Refuse when the trust anchor carries no accepted reference yet
    #[arg(long)]
    require_reference: bool,
    /// New file receiving a copy of the report, verified or refused
    #[arg(long)]
    report_out: Option<PathBuf>,
}

/// Outcome of one invocation: the document for standard output and the exit
/// status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    pub exit_code: u8,
    pub stdout: String,
}

fn render(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| {
        format!(
            "{{\"format\":\"{ERROR_FORMAT}\",\"errors\":[{{\"code\":\"io_failure\",\"message\":\"the result could not be serialized\",\"detail\":{{}},\"action\":\"Retry; report this defect if it persists.\"}}]}}"
        )
    })
}

fn failed(failure: Failure) -> Completed {
    Completed {
        exit_code: failure.exit_code(),
        stdout: render(&json!({ "format": ERROR_FORMAT, "errors": [failure.error] })),
    }
}

/// A successful command document: the common header followed by the fields of
/// the command result.
fn succeeded(command: &str, result: &impl Serialize) -> Completed {
    let mut document = serde_json::Map::new();
    document.insert("format".into(), COMMAND_FORMAT.into());
    document.insert("command".into(), command.into());
    match serde_json::to_value(result) {
        Ok(Value::Object(fields)) => document.extend(fields),
        _ => {
            return failed(Failure::new(
                Code::IoFailure,
                "the command result could not be serialized",
            ));
        }
    }
    Completed {
        exit_code: 0,
        stdout: render(&Value::Object(document)),
    }
}

fn outcome<T: Serialize>(command: &str, result: Result<T, Failure>) -> Completed {
    match result {
        Ok(result) => succeeded(command, &result),
        Err(failure) => failed(failure),
    }
}

fn execute(command: Command) -> Completed {
    match command {
        Command::Key(KeyCommand::Init { key_dir }) => outcome("key init", init_key(&key_dir)),
        Command::Key(KeyCommand::Show { key_dir, out }) => {
            outcome("key show", show_key(&key_dir, out.as_deref()))
        }
        Command::Anchor(AnchorCommand::Init {
            public_key,
            expect_key_id,
            tenant_id,
            namespace_uuid,
            out,
        }) => outcome(
            "anchor init",
            init_anchor(&AnchorInitRequest {
                public_key: &public_key,
                expect_key_id: &expect_key_id,
                tenant_id: &tenant_id,
                namespace_uuid: &namespace_uuid,
                out: &out,
            }),
        ),
        Command::Anchor(AnchorCommand::Advance {
            anchor,
            source,
            data,
            out,
        }) => {
            let advanced = source.source().and_then(|source| {
                advance_anchor(&AnchorAdvanceRequest {
                    anchor: &anchor,
                    source,
                    data: data.as_deref(),
                    out: &out,
                })
            });
            match advanced {
                // A refused verification is reported exactly as `verify` does.
                Ok(advanced) if advanced.status == AdvanceStatus::Refused => Completed {
                    exit_code: 2,
                    stdout: render(&advanced.verification),
                },
                other => outcome("anchor advance", other),
            }
        }
        Command::Anchor(AnchorCommand::ApproveRotation {
            anchor,
            source,
            public_key,
            expect_key_id,
            unendorsed_recovery,
            data,
            out,
        }) => outcome(
            "anchor approve-rotation",
            source.source().and_then(|source| {
                approve_rotation(&RotationApprovalRequest {
                    anchor: &anchor,
                    source,
                    data: data.as_deref(),
                    public_key: &public_key,
                    expect_key_id: &expect_key_id,
                    unendorsed_recovery,
                    out: &out,
                })
            }),
        ),
        Command::Diagnose(arguments) => outcome(
            "diagnose",
            diagnose(&DiagnoseRequest {
                data: arguments
                    .data
                    .as_deref()
                    .zip(arguments.tenant_id.as_deref()),
                checkpoint_dir: arguments.checkpoint_dir.as_deref(),
                key_dir: arguments.key_dir.as_deref(),
                anchor: arguments.anchor.as_deref(),
            }),
        ),
        Command::Export(arguments) => outcome(
            "export",
            export_bundle(&ExportRequest {
                checkpoint_dir: &arguments.checkpoint_dir,
                out: &arguments.out,
                from_log_index: arguments.from_log_index,
                through_log_index: arguments.through_log_index,
            }),
        ),
        Command::RotateKey(arguments) => {
            let retiring = match (&arguments.key_dir, &arguments.retiring_key_id) {
                (Some(key_dir), None) => RetiringKey::Endorsing { key_dir },
                (None, Some(retiring_key_id)) if arguments.unendorsed_recovery => {
                    RetiringKey::UnendorsedRecovery { retiring_key_id }
                }
                _ => {
                    return failed(Failure::new(
                        Code::InvalidArgument,
                        "name either the retiring key directory, or --unendorsed-recovery with --retiring-key-id",
                    ));
                }
            };
            outcome(
                "rotate-key",
                rotate_key(&RotateRequest {
                    checkpoint_dir: &arguments.checkpoint_dir,
                    new_key_dir: &arguments.new_key_dir,
                    retiring,
                }),
            )
        }
        Command::Create(arguments) => outcome(
            "create",
            create_checkpoint(&CreateRequest {
                data: &arguments.data,
                tenant_id: &arguments.tenant_id,
                key_dir: &arguments.key_dir,
                checkpoint_dir: &arguments.checkpoint_dir,
                new_log: arguments.new_log,
                previous_record: arguments.previous_record.as_deref(),
            }),
        ),
        Command::Verify(arguments) => match arguments.source.source() {
            Ok(source) => verification(&VerifyRequest {
                anchor: &arguments.anchor,
                source,
                data: arguments.data.as_deref(),
                require_reference: arguments.require_reference,
                report_out: arguments.report_out.as_deref(),
            }),
            Err(failure) => failed(failure),
        },
    }
}

/// Run one verification and render it as the `verify` command prints it: the
/// report with exit status 0 when it is verified and 2 when it is refused, or
/// an error document with exit status 1 or 2 when no report could be made.
pub fn verification(request: &VerifyRequest<'_>) -> Completed {
    match verify(request) {
        Ok(report) => Completed {
            exit_code: if report.is_verified() { 0 } else { 2 },
            stdout: render(&report),
        },
        Err(failure) => failed(failure),
    }
}

/// Parse and run one invocation. `arguments` includes the program name.
pub fn run<I, T>(arguments: I) -> Completed
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(arguments) {
        Ok(cli) => execute(cli.command),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Completed {
                exit_code: 0,
                stdout: error.render().to_string().trim_end().to_owned(),
            }
        }
        Err(error) => {
            let entry: ErrorEntry = Failure::new(
                Code::InvalidArgument,
                "the command line is not valid; run with --help for the commands",
            )
            .with("usage", error.render().to_string())
            .error;
            Completed {
                exit_code: 1,
                stdout: render(&json!({ "format": ERROR_FORMAT, "errors": [entry] })),
            }
        }
    }
}
