use agentsandbox::mysql_storage::{
    MysqlOwnedPreparationRefusal, MysqlOwnedPreparationRequest, MysqlProbeError,
    publish_owned_fresh_source,
};
use anyhow::{Context, Result, anyhow};
use clap::Args;
use std::{
    io::{self, Write},
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Args)]
pub(crate) struct PrepareMysqlOwnedArgs {
    /// Private tenant registry containing the dedicated MySQL operator profile.
    #[arg(long)]
    registry: PathBuf,
    /// Exact tenant whose MySQL database, account and namespace the registry selects.
    #[arg(long)]
    tenant: String,
    /// Exact owned audit format (14 or 15).
    #[arg(long, value_parser = parse_schema_version)]
    schema_version: u16,
    /// Expected UUID of the physical MySQL server.
    #[arg(long, value_parser = parse_uuid)]
    server_uuid: Uuid,
    /// Stable identity for this publication operation.
    #[arg(long, value_parser = parse_uuid)]
    operation_id: Uuid,
    /// Stable identity of the installation.
    #[arg(long, value_parser = parse_uuid)]
    installation_id: Uuid,
    /// Stable identity assigned to this source.
    #[arg(long, value_parser = parse_uuid)]
    source_id: Uuid,
}

fn parse_schema_version(value: &str) -> std::result::Result<u16, &'static str> {
    match value {
        "14" => Ok(14),
        "15" => Ok(15),
        _ => Err("expected owned format 14 or 15"),
    }
}

fn parse_uuid(value: &str) -> std::result::Result<Uuid, &'static str> {
    let invalid = "expected a canonical non-nil UUID";
    let uuid = Uuid::parse_str(value).map_err(|_| invalid)?;
    if uuid.is_nil() || uuid.to_string() != value {
        return Err(invalid);
    }
    Ok(uuid)
}

pub(crate) async fn run(args: PrepareMysqlOwnedArgs) -> Result<()> {
    let target = crate::gateway_config::load_mysql_preparation_target(&args.registry, &args.tenant)
        .map_err(|_| {
            anyhow!(
                "Cannot load MySQL operator configuration; check the private registry, selected tenant and private authentication files"
            )
        })?;
    let receipt = publish_owned_fresh_source(
        target.endpoint(),
        target.credentials(),
        MysqlOwnedPreparationRequest {
            tenant_id: &args.tenant,
            namespace_id: target.namespace,
            schema_version: args.schema_version,
            server_uuid: args.server_uuid,
            operation_id: args.operation_id,
            installation_id: args.installation_id,
            source_id: args.source_id,
        },
    )
    .await
    .map_err(|error| {
        if let Some(refusal) = error.downcast_ref::<MysqlOwnedPreparationRefusal>() {
            return (*refusal).into();
        }
        if let Some(refusal) = error.downcast_ref::<MysqlProbeError>() {
            return (*refusal).into();
        }
        anyhow!(
            "MySQL owned publication could not be verified; preserve the target and check its exact configuration and state"
        )
    })?;

    let json = serde_json::to_string(&receipt).context("cannot encode the verified receipt")?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{json}")
        .and_then(|()| stdout.flush())
        .context("MySQL publication verified but the receipt could not be written")
}
