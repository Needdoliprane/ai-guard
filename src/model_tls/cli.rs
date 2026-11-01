use super::{Failure, OutputPlan, Sources, VolumeSelection, decode_ack, publish};
use crate::runtime::{
    RuntimeKind,
    model_tls::{Attempt, Cancellation, CleanupLocalStatus, ProvisionRuntime, image_id},
};
use clap::{Args, Subcommand};
use std::{future::Future, path::PathBuf, time::Duration};
use tokio::time::Instant;

#[derive(Args)]
pub struct ModelTlsArgs {
    #[command(subcommand)]
    command: ModelTlsCommand,
}
#[derive(Subcommand)]
enum ModelTlsCommand {
    Provision(ProvisionArgs),
}
#[derive(Args)]
struct ProvisionArgs {
    #[arg(long)]
    ca_file: PathBuf,
    #[arg(long)]
    cert_file: PathBuf,
    #[arg(long)]
    key_file: PathBuf,
    #[arg(long)]
    server_name: String,
    #[arg(long)]
    image: String,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long, value_enum, default_value = "docker")]
    runtime: RuntimeKind,
    #[arg(long)]
    runtime_program: Option<PathBuf>,
    #[arg(long)]
    runtime_endpoint: Option<String>,
    #[arg(long)]
    allow_rootful_runtime: bool,
}
pub async fn execute(
    arguments: ModelTlsArgs,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let ModelTlsCommand::Provision(arguments) = arguments.command;
    if !image_id(&arguments.image) {
        return Err(Failure::InvalidInput.into());
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let sources = Sources::open(
        &arguments.ca_file,
        &arguments.cert_file,
        &arguments.key_file,
        arguments.server_name,
    )?;
    let generation = sources.expected.generation.clone();
    let output = OutputPlan::prepare(&arguments.output_dir)?;
    let mut attempt = Attempt {
        generation: generation.clone(),
        preparer: format!("ai-guard-tls-preparer-{generation}"),
        cid: None,
        volumes: VolumeSelection::new(&generation),
        stop_observation: "not_created",
        cleanup_local: CleanupLocalStatus::NotAttempted,
        runtime: None,
    };
    let cancel = Cancellation::default();
    tokio::pin!(shutdown);
    let result = {
        let work = async {
            let runtime = ProvisionRuntime::connect(
                arguments.runtime,
                arguments.runtime_program.as_deref(),
                arguments.runtime_endpoint.as_deref(),
                arguments.allow_rootful_runtime,
                deadline,
                cancel.clone(),
            )
            .await?;
            attempt.runtime = Some(runtime.provenance()?);
            let result = async {
                runtime.inspect_image(&arguments.image).await?;
                let ca_volume = runtime
                    .create_volume(&attempt.volumes.ca, &generation, "ca")
                    .await?;
                let identity_volume = runtime
                    .create_volume(&attempt.volumes.identity, &generation, "identity")
                    .await?;
                runtime
                    .create_preparer(&arguments.image, &mut attempt)
                    .await?;
                sources.revalidate()?;
                let wire = sources.wire()?;
                let bytes = runtime
                    .import(&arguments.image, &mut attempt, &wire)
                    .await?;
                decode_ack(&bytes, &sources.expected)?;
                sources.revalidate()?;
                let ca_current = runtime
                    .volume(&attempt.volumes.ca, &generation, "ca")
                    .await?;
                let identity_current = runtime
                    .volume(&attempt.volumes.identity, &generation, "identity")
                    .await?;
                if ca_current != ca_volume || identity_current != identity_volume {
                    return Err(Failure::Runtime);
                }
                let provenance = runtime.revalidate_provenance().await?;
                if attempt.runtime.as_ref() != Some(&provenance) {
                    return Err(Failure::Runtime);
                }
                cancel.check()?;
                if Instant::now() >= deadline {
                    return Err(Failure::TimedOut);
                }
                publish(
                    &output,
                    &sources.expected,
                    &arguments.image,
                    &attempt.volumes,
                    &provenance,
                )
            }
            .await;
            match result {
                Err(primary) => {
                    let cleanup = runtime
                        .stop_after_failure(&arguments.image, &mut attempt)
                        .await;
                    Err(attempt.finish_failure(primary, cleanup))
                }
                success => success,
            }
        };
        tokio::pin!(work);
        tokio::select! {
            result=&mut work=>result,
            _=&mut shutdown=>{cancel.cancel();work.await},
        }
    };
    match result {
        Ok(_path) => {
            println!(
                "TLS material provisioned in the explicit output directory. Use volumes.env only after this successful exit; retained generation {generation}."
            );
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "TLS provisioning refused: {error}; retained identifiers {}",
                serde_json::to_string(&attempt).unwrap_or_else(|_| "unavailable".into())
            );
            Err(anyhow::anyhow!("model TLS provision refused"))
        }
    }
}
