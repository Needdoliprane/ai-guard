//! `human-sources human-rights`: offline in-place installation of every
//! pending identity marker (today `human_rights_v2`) on one adopted owned
//! tenant Store14/15, with a verified backup and a resumable journal.
//!
//! The command opens the union through the maintenance guard, which takes the
//! real owner locks: a running gateway makes it refuse. Standard output is one
//! closed JSON report; a refusal is one closed, secret-free line.
use super::activation::load_identity_marker_inputs;
use agentsandbox::human_authority::migration::operator::{
    OwnedIdentityMarkersRefusalV1, OwnedIdentityMarkersReportV1, OwnedIdentityMarkersRequest,
    install_owned_identity_markers,
};
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct HumanRightsArgs {
    #[command(subcommand)]
    command: HumanRightsCommand,
}

#[derive(Subcommand)]
enum HumanRightsCommand {
    /// Back up the source, journal the intent and append every pending marker.
    Install(HumanRightsRunArgs),
    /// Finish or re-confirm the maintenance recorded in an existing directory.
    Resume(HumanRightsRunArgs),
}

#[derive(Args)]
struct HumanRightsRunArgs {
    /// Published private owned-source activation (`activation.json`).
    #[arg(long)]
    human_sources_activation: PathBuf,
    /// Adopted tenant whose Store14/15 receives the markers.
    #[arg(long)]
    tenant_id: String,
    /// New private directory (install) or the same directory (resume), outside
    /// the preparation directory and every source directory.
    #[arg(long)]
    maintenance_dir: PathBuf,
    /// Operator attestation: gateway, dashboard, ingress and brokers are stopped.
    /// The owner locks are still taken; this is not fencing of an older process.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

fn execute(args: &HumanRightsRunArgs, resume: bool) -> Result<OwnedIdentityMarkersReportV1> {
    if !args.confirm_control_stopped {
        bail!("human_rights_control_stop_attestation_required");
    }
    let inputs = load_identity_marker_inputs(&args.human_sources_activation, &args.tenant_id)
        .map_err(|error| {
            if error
                .chain()
                .any(|cause| cause.to_string() == "human_rights_source_unknown")
            {
                anyhow::Error::from(OwnedIdentityMarkersRefusalV1::SourceUnknown)
            } else {
                anyhow::anyhow!("human_rights_activation_refused")
            }
        })?;
    install_owned_identity_markers(OwnedIdentityMarkersRequest {
        manifest: inputs.manifest,
        configuration: inputs.configuration,
        files: inputs.files,
        tenant_id: args.tenant_id.clone(),
        redactor: inputs.redactor,
        max_events: inputs.max_events,
        maintenance_directory: args.maintenance_dir.clone(),
        excluded_directories: vec![inputs.preparation_directory],
        resume,
    })
}

pub(crate) fn run(args: HumanRightsArgs) -> Result<()> {
    let (step, resume, args) = match args.command {
        HumanRightsCommand::Install(args) => ("install", false, args),
        HumanRightsCommand::Resume(args) => ("resume", true, args),
    };
    match execute(&args, resume) {
        Ok(report) => {
            println!("{}", serde_json::to_string(&report)?);
            Ok(())
        }
        Err(error) => {
            // The refusal is the outermost context or the error itself.
            let state = error
                .downcast_ref::<OwnedIdentityMarkersRefusalV1>()
                .map(|refusal| refusal.code())
                .unwrap_or_else(|| {
                    if error.to_string() == "human_rights_control_stop_attestation_required" {
                        "human_rights_control_stop_attestation_required"
                    } else {
                        "human_rights_activation_refused"
                    }
                });
            bail!(
                "human_rights_step_refused; step={step}; state={state}; action=preserve_maintenance_directory_and_sources"
            )
        }
    }
}
