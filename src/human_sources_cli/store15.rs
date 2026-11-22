use super::*;
use crate::gateway_config::{ConfiguredTenant, load_owned_tenants};
use activation::ExplicitOwnedSourcesArgs;
use agentsandbox::human_authority::{HumanAuthorityFailure, startup::HumanOwnedStartupRequest};

#[derive(Args)]
pub(crate) struct UpgradeEgressArgs {
    /// Published private initial owned-source activation.
    #[arg(
        long,
        conflicts_with_all = [
            "human_sources_manifest",
            "human_credentials_envelope",
            "tenants_file",
            "human_control_config_revision",
            "oidc_config",
            "human_oidc_mapping_revision",
            "max_events",
        ]
    )]
    human_sources_activation: Option<PathBuf>,
    #[command(flatten)]
    explicit: ExplicitOwnedSourcesArgs,
    /// Tenant Store to migrate from the selected closure.
    #[arg(long)]
    tenant: String,
    /// Stable operation UUID retained for resume.
    #[arg(long)]
    operation_id: Uuid,
    /// Fresh private destination. With --resume, reopen the same directory.
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long)]
    resume: bool,
    /// Attest that the controls and consumers are stopped before maintenance.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

pub(super) fn run(args: UpgradeEgressArgs) -> Result<()> {
    ensure!(
        args.confirm_control_stopped,
        "store15_control_stop_attestation_required"
    );
    let (prior, tenants) =
        resolve_prior(&args).map_err(|_| anyhow::anyhow!("store15_prior_closure_refused"))?;
    let credential_roles = tenants
        .iter()
        .map(|tenant| {
            (
                tenant.tenant_id.as_str(),
                tenant.runner_token.as_str(),
                tenant.viewer_token.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let result = operator::upgrade_store15(operator::Store15UpgradeRequest {
        prior,
        configured_credentials: &credential_roles,
        tenant: &args.tenant,
        operation_id: args.operation_id,
        output_directory: &args.output_dir,
        resume: args.resume,
    });
    match result {
        Ok(result) => {
            println!(
                "store15_closure_published closure_revision={} genesis_sequence={} readiness_not_checked",
                result.closure_revision, result.genesis_sequence
            );
            Ok(())
        }
        Err(error) => {
            if let Some(refusal) = error.downcast_ref::<operator::Store15UpgradeRefusal>() {
                bail!("{refusal}");
            }
            if error.downcast_ref::<HumanAuthorityFailure>()
                == Some(&HumanAuthorityFailure::OwnerContended)
            {
                bail!("store15_control_active preserve_output retry_after_control_stops");
            }
            bail!("store15_preparation_refused preserve_output verify_inputs_and_resume")
        }
    }
}

fn resolve_prior(
    args: &UpgradeEgressArgs,
) -> Result<(HumanOwnedStartupRequest, Vec<ConfiguredTenant>)> {
    if let Some(path) = &args.human_sources_activation {
        let loaded = load_human_sources_activation(path)?;
        return Ok((loaded.request, loaded.tenants));
    }
    let tenants = load_owned_tenants(
        args.explicit
            .tenants_file
            .as_deref()
            .context("Store15 tenant registry missing")?,
    )?;
    let (loaded, _, _) = args.explicit.resolve_for_maintenance()?;
    let (prior, _) = loaded.into_startup_request()?;
    Ok((prior, tenants))
}
