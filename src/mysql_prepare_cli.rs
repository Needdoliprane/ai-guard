use crate::gateway_config::{MysqlPreparationProfileRefusal, load_mysql_preparation_target};
use agentsandbox::mysql_storage::{
    MysqlOwnedPreparationRefusal, MysqlOwnedPreparationRequest, MysqlProbeError,
    publish_owned_fresh_source, verify_owned_audit_chain, verify_owned_fresh_source,
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
    /// Private tenant registry supplied to this installation's gateway at startup.
    #[arg(long)]
    gateway_registry: PathBuf,
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
    /// Verify the exact fresh publication in read-only mode. Refuse an empty schema.
    #[arg(long, conflicts_with = "verify_chain")]
    verify_only: bool,
    /// Verify the exact owned audit manifest, anchor and chain without mutations.
    #[arg(long, conflicts_with = "verify_only")]
    verify_chain: bool,
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
    let target = load_mysql_preparation_target(&args.registry, &args.gateway_registry, &args.tenant)
        .map_err(|error| {
            if let Some(refusal) = error.downcast_ref::<MysqlPreparationProfileRefusal>() {
                return (*refusal).into();
            }
            anyhow!(
                "Cannot load MySQL publication configuration; check both private registries, the selected tenant and the operator authentication files"
            )
        })?;
    let request = MysqlOwnedPreparationRequest {
        tenant_id: &args.tenant,
        namespace_id: target.namespace,
        schema_version: args.schema_version,
        server_uuid: args.server_uuid,
        operation_id: args.operation_id,
        installation_id: args.installation_id,
        source_id: args.source_id,
    };
    let endpoint = target.endpoint();
    let credentials = target.credentials();
    let json = if args.verify_chain {
        let receipt = verify_owned_audit_chain(endpoint, credentials, request)
            .await
            .map_err(|error| {
                owned_operation_error(
                    error,
                    "MySQL owned audit chain verification failed; preserve the target and check its exact configuration and state",
                )
            })?;
        serde_json::to_string(&receipt).context("cannot encode the verified chain receipt")?
    } else {
        let receipt = if args.verify_only {
            verify_owned_fresh_source(endpoint, credentials, request).await
        } else {
            publish_owned_fresh_source(endpoint, credentials, request).await
        }
        .map_err(|error| {
            owned_operation_error(
                error,
                "MySQL owned publication could not be verified; preserve the target and check its exact configuration and state",
            )
        })?;
        serde_json::to_string(&receipt).context("cannot encode the verified receipt")?
    };
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{json}")
        .and_then(|()| stdout.flush())
        .context(if args.verify_chain {
            "MySQL audit chain verified but the receipt could not be written"
        } else {
            "MySQL publication verified but the receipt could not be written"
        })
}

fn owned_operation_error(error: anyhow::Error, fallback: &'static str) -> anyhow::Error {
    if let Some(refusal) = error.downcast_ref::<MysqlOwnedPreparationRefusal>() {
        return (*refusal).into();
    }
    if let Some(refusal) = error.downcast_ref::<MysqlProbeError>() {
        return (*refusal).into();
    }
    anyhow!(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_operation_error_preserves_only_safe_diagnostics() {
        let fallback = "verification failed";
        let private_marker = "private endpoint marker";
        let probe = anyhow!(MysqlProbeError::TimedOut).context(private_marker);
        let preparation = anyhow!(MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed)
            .context(private_marker);

        for (source, expected) in [
            (probe, MysqlProbeError::TimedOut.to_string()),
            (
                preparation,
                MysqlOwnedPreparationRefusal::CatalogVisibilityUnconfirmed.to_string(),
            ),
            (anyhow!(private_marker), fallback.to_owned()),
        ] {
            assert_eq!(
                format!("{:#}", owned_operation_error(source, fallback)),
                expected
            );
        }
    }
}
