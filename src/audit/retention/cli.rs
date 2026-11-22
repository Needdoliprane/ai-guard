//! `agentsandbox audit-retention`: offline operator commands. Each prints
//! exactly one JSON document on standard output. Exit status 0 is success, 2
//! a refusal with a closed code (nothing changed), 1 an operational error.
use super::{
    failure::{RetentionFailure, code, refusal},
    policy::AuditRetentionPolicyV1,
    store::RetentionApplyRequest,
};
use crate::{audit::Store, redaction::Redactor};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf};

pub const COMMAND_FORMAT: &str = "ai-guard.audit.retention-command.v1";
pub const ERROR_FORMAT: &str = "ai-guard.audit.retention-error.v1";

#[derive(Args, Debug)]
pub struct AuditRetentionArgs {
    #[command(subcommand)]
    command: RetentionCommand,
}

#[derive(Subcommand, Debug)]
enum RetentionCommand {
    /// Install the retention capability once, after a verified new backup.
    Install(InstallArgs),
    /// Show, without effect, the next segment that the policy admits.
    Plan(PlanArgs),
    /// Archive a reviewed segment into a new directory, then reclaim its leaf events.
    Apply(ApplyArgs),
    /// Verify the archived prefix end to end, and any export, against the live store.
    Verify(VerifyArgs),
    /// Write a verifiable export of a contiguous live range into a new directory.
    Export(ExportArgs),
}

#[derive(Args, Debug)]
struct StoreArgs {
    /// Existing private SQLite tenant store.
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    tenant_id: String,
    /// The audit event bound the gateway uses for this store.
    #[arg(long, default_value_t = 100_000)]
    max_events: u64,
    /// The payload byte cap the gateway uses for this store (`--audit-max-bytes`).
    #[arg(long, default_value_t = crate::audit::DEFAULT_PHYSICAL_BYTES)]
    audit_max_bytes: u64,
}

/// Every reclaim keeps the signed checkpoint positions of the tenant live, so
/// that `ai-guard-audit-checkpoint` keeps comparing the store with them. The
/// operator names the checkpoint directories or attests that there are none.
#[derive(Args, Debug)]
struct CheckpointArgs {
    /// Checkpoint directory of this tenant; repeat for continuation directories.
    #[arg(
        long,
        required_unless_present = "without_signed_checkpoints",
        conflicts_with = "without_signed_checkpoints"
    )]
    checkpoint_dir: Vec<PathBuf>,
    /// Operator attestation: this tenant has no signed audit checkpoint.
    #[arg(long)]
    without_signed_checkpoints: bool,
}

#[derive(Args, Debug)]
struct InstallArgs {
    #[command(flatten)]
    store: StoreArgs,
    /// New private backup path; an existing destination is never overwritten.
    #[arg(long)]
    backup: PathBuf,
    /// Operator attestation: gateway, dashboard and brokers are stopped; not runtime fencing.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

#[derive(Args, Debug)]
struct PlanArgs {
    #[command(flatten)]
    store: StoreArgs,
    /// Private retention policy file.
    #[arg(long)]
    policy: PathBuf,
    /// End the segment here instead of at the highest admitted sequence.
    #[arg(long)]
    through_seq: Option<i64>,
    #[command(flatten)]
    checkpoints: CheckpointArgs,
}

#[derive(Args, Debug)]
struct ApplyArgs {
    #[command(flatten)]
    store: StoreArgs,
    #[arg(long)]
    policy: PathBuf,
    /// The `last_seq` of the reviewed plan.
    #[arg(long)]
    through_seq: i64,
    /// The `plan_sha256` of the reviewed plan.
    #[arg(long)]
    expect_plan: String,
    /// New archive directory under an existing private parent.
    #[arg(long)]
    archive_dir: PathBuf,
    #[command(flatten)]
    checkpoints: CheckpointArgs,
    /// Operator attestation: gateway, dashboard and brokers are stopped; not runtime fencing.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

#[derive(Args, Debug)]
struct VerifyArgs {
    #[command(flatten)]
    store: StoreArgs,
    /// Archive directory; repeat for every segment and export to examine.
    #[arg(long)]
    archive_dir: Vec<PathBuf>,
}

#[derive(Args, Debug)]
struct ExportArgs {
    #[command(flatten)]
    store: StoreArgs,
    #[arg(long)]
    archive_dir: PathBuf,
    /// First exported sequence; defaults to the first event after the archived prefix.
    #[arg(long)]
    first_seq: Option<i64>,
    /// Last exported sequence; defaults to the verified head.
    #[arg(long)]
    last_seq: Option<i64>,
}

pub struct Completed {
    pub exit_code: i32,
    pub stdout: String,
}

fn open(args: &StoreArgs) -> anyhow::Result<Store> {
    anyhow::ensure!(
        (1..=1_000_000).contains(&args.max_events),
        RetentionFailure::new(code::RANGE_INVALID, "--max-events must be 1 to 1000000")
    );
    Store::open_existing_for_tenant(
        &args.data,
        &args.tenant_id,
        Redactor::new(vec![]),
        args.max_events,
    )?
    .with_physical_bytes(args.audit_max_bytes)
}

fn kept_positions(args: &CheckpointArgs, tenant_id: &str) -> anyhow::Result<BTreeSet<i64>> {
    anyhow::ensure!(
        args.without_signed_checkpoints != !args.checkpoint_dir.is_empty(),
        RetentionFailure::new(
            code::RANGE_INVALID,
            "name the tenant checkpoint directories or attest --without-signed-checkpoints"
        )
    );
    let mut kept = BTreeSet::new();
    for directory in &args.checkpoint_dir {
        let positions = crate::audit::signed_checkpoint::signed_positions(directory, tenant_id)
            .map_err(|failure| {
                RetentionFailure::new(
                    code::CHECKPOINT_LOG_UNAVAILABLE,
                    "a checkpoint directory could not be read; nothing was changed",
                )
                .with("checkpoint_dir", directory.display().to_string())
                .with("cause", failure.to_string())
            })?;
        for position in positions {
            kept.insert(i64::try_from(position)?);
        }
    }
    Ok(kept)
}

fn command(name: &str, mut body: Value) -> Value {
    if let Value::Object(map) = &mut body {
        map.insert("format".into(), json!(COMMAND_FORMAT));
        map.insert("command".into(), json!(name));
    }
    body
}

fn execute(args: AuditRetentionArgs) -> anyhow::Result<(i32, Value)> {
    match args.command {
        RetentionCommand::Install(args) => {
            anyhow::ensure!(
                args.confirm_control_stopped,
                "stop the control plane and all brokers before installing retention"
            );
            let store = open(&args.store)?;
            let report = store.install_audit_retention(&args.backup)?;
            Ok((
                0,
                command(
                    "install",
                    json!({
                        "tenant_id": args.store.tenant_id,
                        "backup_path": report.backup_path,
                        "backup_head_seq": report.backup_head_seq,
                        "backup_head_hash": report.backup_head_hash,
                        "genesis_seq": report.genesis_seq,
                        "genesis_hash": report.genesis_hash,
                    }),
                ),
            ))
        }
        RetentionCommand::Plan(args) => {
            let policy = AuditRetentionPolicyV1::load(&args.policy)?;
            let kept = kept_positions(&args.checkpoints, &args.store.tenant_id)?;
            let store = open(&args.store)?;
            let planned =
                store.audit_retention_plan(&policy, chrono::Utc::now(), args.through_seq, &kept)?;
            Ok((0, command("plan", serde_json::to_value(planned)?)))
        }
        RetentionCommand::Apply(args) => {
            anyhow::ensure!(
                args.confirm_control_stopped,
                "stop the control plane and all brokers before applying retention"
            );
            let policy = AuditRetentionPolicyV1::load(&args.policy)?;
            let kept = kept_positions(&args.checkpoints, &args.store.tenant_id)?;
            let store = open(&args.store)?;
            let receipt = store.apply_audit_retention(&RetentionApplyRequest {
                policy: &policy,
                now: chrono::Utc::now(),
                through_seq: args.through_seq,
                expect_plan_sha256: &args.expect_plan,
                archive_dir: &args.archive_dir,
                kept_positions: &kept,
            })?;
            Ok((0, command("apply", serde_json::to_value(receipt)?)))
        }
        RetentionCommand::Verify(args) => {
            let store = open(&args.store)?;
            let report = store.verify_audit_archives(&args.archive_dir)?;
            let exit = if report.verdict == "verified" { 0 } else { 2 };
            Ok((exit, serde_json::to_value(report)?))
        }
        RetentionCommand::Export(args) => {
            let store = open(&args.store)?;
            let archive =
                store.export_audit_archive(&args.archive_dir, args.first_seq, args.last_seq)?;
            Ok((0, command("export", json!({ "archive": archive }))))
        }
    }
}

/// Run one command; never panics on a refusal and never prints a secret.
pub fn run(args: AuditRetentionArgs) -> Completed {
    let (exit_code, document) = match execute(args) {
        Ok(done) => done,
        Err(error) => match refusal(&error) {
            Some(refused) => (
                if refused.code == code::COMMIT_UNCONFIRMED {
                    1
                } else {
                    2
                },
                json!({
                    "format": ERROR_FORMAT,
                    "code": refused.code,
                    "message": refused.message,
                    "detail": refused.detail,
                }),
            ),
            None => (
                1,
                json!({
                    "format": ERROR_FORMAT,
                    "code": "operational_error",
                    "message": format!("{error:#}"),
                    "detail": {},
                }),
            ),
        },
    };
    Completed {
        exit_code,
        stdout: serde_json::to_string_pretty(&document).expect("serializable document"),
    }
}
