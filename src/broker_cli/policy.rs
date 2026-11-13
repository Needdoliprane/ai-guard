//! Explicit operator policy requests; no broker lifecycle or network enforcement.
mod storage;
#[cfg(test)]
mod tests;

use agentsandbox::{
    control::egress_policy::{EgressPolicyReceiptV1, EgressPolicyStateV1},
    control_fs::{PrivateFile, absolute_path},
    runner::AuditClient,
};
use anyhow::{Result, anyhow, ensure};
use clap::Subcommand;
use std::{io::Write, path::PathBuf};
use storage::{Loaded, Phase, Request};

#[derive(Subcommand)]
pub(super) enum PolicyCommand {
    /// Prepare one immutable request in a new private directory, without network IO.
    Prepare {
        #[arg(long)]
        gateway: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Attempt the prepared request once after authenticated control preflight.
    Apply {
        #[arg(long)]
        operation_dir: PathBuf,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Explicitly recover the same operation; never replace its scope or CAS.
    Resume {
        #[arg(long)]
        operation_dir: PathBuf,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Read local historical evidence, without contacting the control.
    Status {
        #[arg(long)]
        operation_dir: PathBuf,
    },
}

pub(super) async fn execute(command: PolicyCommand) -> Result<()> {
    match command {
        PolicyCommand::Prepare {
            gateway,
            input,
            output_dir,
        } => {
            let loaded = storage::prepare(&gateway, &input, &output_dir)?;
            emit(&loaded, None)
        }
        PolicyCommand::Status { operation_dir } => emit(&Loaded::open(&operation_dir)?, None),
        PolicyCommand::Apply {
            operation_dir,
            token_file,
        } => attempt(operation_dir, token_file, false).await,
        PolicyCommand::Resume {
            operation_dir,
            token_file,
        } => attempt(operation_dir, token_file, true).await,
    }
}

async fn attempt(path: PathBuf, token_path: PathBuf, resume: bool) -> Result<()> {
    let mut loaded = Loaded::open(&path)?;
    if resume {
        loaded.recover_pending()?;
    } else {
        ensure!(
            loaded.state.phase == Phase::Prepared && !loaded.has_pending(),
            "egress_policy_resume_required"
        );
    }
    let token_path =
        absolute_path(&token_path).map_err(|_| anyhow!("egress_policy_unsafe_token"))?;
    let token =
        PrivateFile::open(&token_path, 4096).map_err(|_| anyhow!("egress_policy_unsafe_token"))?;
    let credential = std::str::from_utf8(token.bytes())
        .map_err(|_| anyhow!("egress_policy_invalid_token"))?
        .trim();
    token
        .revalidate()
        .map_err(|_| anyhow!("egress_policy_token_changed"))?;
    loaded.revalidate()?;
    if let Some(receipt) = loaded.receipt()? {
        ensure!(resume, "egress_policy_resume_required");
        loaded.acknowledge(&receipt)?;
        token
            .revalidate()
            .map_err(|_| anyhow!("egress_policy_token_changed"))?;
        return emit(&loaded, Some(credential));
    }
    let client = AuditClient::new(&loaded.metadata.gateway, credential)
        .map_err(|_| anyhow!("egress_policy_invalid_origin_or_token"))?;
    let diagnostic = client
        .control_supervision_identity()
        .await
        .map_err(|_| anyhow!("egress_policy_preflight_unavailable"))?;
    token
        .revalidate()
        .map_err(|_| anyhow!("egress_policy_token_changed"))?;
    loaded.revalidate()?;
    let scope = loaded.request.scope();
    ensure!(diagnostic.role == "runner", "egress_policy_forbidden");
    ensure!(
        diagnostic.tenant_id.as_str() == scope.tenant_id()
            && diagnostic.control_namespace_uuid.as_deref() == Some(scope.control_namespace_uuid()),
        "egress_policy_scope_conflict"
    );
    ensure!(
        diagnostic.schema_version == 15,
        "egress_policy_migration_required"
    );
    // active is descriptive, never a substitute for the actual POST authority.
    loaded.begin_attempt()?;
    token
        .revalidate()
        .map_err(|_| anyhow!("egress_policy_token_changed"))?;
    loaded.revalidate()?;
    let result = match &loaded.request {
        Request::Replace(request) => client.egress_replace_policy(request).await,
        Request::Revoke(request) => client.egress_revoke_policy(request).await,
    };
    // A refusal here may follow a remote commit: keep the durable unresolved intent.
    token
        .revalidate()
        .map_err(|_| anyhow!("egress_policy_token_changed_after_attempt"))?;
    loaded.revalidate()?;
    match result {
        Ok(receipt) => {
            loaded.acknowledge(&receipt)?;
            emit(&loaded, Some(credential))
        }
        Err(error) => {
            loaded.failed_attempt(error.code())?;
            emit(&loaded, Some(credential))?;
            Err(anyhow!(error.code()))
        }
    }
}

fn output(loaded: &Loaded) -> Result<Vec<u8>> {
    loaded.revalidate()?;
    let receipt = loaded.receipt()?.map(|receipt: EgressPolicyReceiptV1| serde_json::json!({
        "revision": receipt.revision(), "before_epoch": receipt.before_epoch(), "after_epoch": receipt.after_epoch(),
        "state": match receipt.state() { EgressPolicyStateV1::Active => "active", EgressPolicyStateV1::RevokeRequested => "revoke_requested" },
        "event_seq": receipt.event_seq(),
    }));
    let value = serde_json::json!({"version": 1, "operation_id": loaded.request.operation_id(), "action": loaded.metadata.action,
        "phase": loaded.state.phase, "attempts": loaded.state.attempts, "last_error": loaded.state.last_error,
        "receipt_present": receipt.is_some(), "pending_state": loaded.has_pending(), "receipt": receipt,
        "historical": true, "coverage": "policy_registry_only"});
    let mut bytes =
        serde_json::to_vec(&value).map_err(|_| anyhow!("egress_policy_output_unresolved"))?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn emit(loaded: &Loaded, credential: Option<&str>) -> Result<()> {
    emit_to(loaded, credential, &mut std::io::stdout().lock())
}
fn emit_to(loaded: &Loaded, credential: Option<&str>, writer: &mut impl Write) -> Result<()> {
    let bytes = output(loaded)?;
    if let Some(secret) = credential.filter(|secret| !secret.is_empty()) {
        ensure!(
            !bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()),
            "egress_policy_output_unresolved"
        );
    }
    writer
        .write_all(&bytes)
        .map_err(|_| anyhow!("egress_policy_output_unresolved"))
}
