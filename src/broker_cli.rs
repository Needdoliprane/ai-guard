use agentsandbox::{
    broker::service::{
        self, RequestedExecutionV1, SubmitV1,
        protocol::{BrokerCommandV1, BrokerReplyV1},
    },
    control_fs::{PrivateFile, absolute_path},
    provider::AuthPreference,
    redaction::Redactor,
    runner::AuditClient,
};
use anyhow::{Result, anyhow, ensure};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Args)]
pub struct BrokerArgs {
    #[command(subcommand)]
    command: BrokerCommand,
}

#[derive(Subcommand)]
enum BrokerCommand {
    /// Read authenticated control metadata without starting or activating a broker.
    ControlInfo {
        #[arg(long)]
        gateway: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Run the explicitly configured trusted local broker in the foreground.
    Serve {
        #[arg(long)]
        config: PathBuf,
        /// Restore only proven STOP targets; never open admission or initialize auth.
        #[arg(long)]
        recovery_only: bool,
    },
    /// Persist a new batch job. The broker continues after this client exits.
    Submit(SubmitArgs),
    /// Read cached journal/control facts for this identity's job.
    Status(OperationArgs),
    /// Read confirmed recovery metadata, including when business loading fails.
    /// Does not inspect the runtime or establish that a worker has stopped.
    RecoveryStatus(OperationArgs),
    /// Discover accepted job UUIDs from confirmed journal metadata, even in STOP-only mode.
    /// No runtime probe or recovery action is performed.
    /// Cold discovery requires a broker explicitly started with serve --recovery-only.
    RecoveryInventory {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        after: Option<Uuid>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// List this profile's journal without hiding incomplete or corrupt data.
    List {
        #[command(flatten)]
        connection: ConnectionArgs,
        #[arg(long)]
        after: Option<Uuid>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Submit a human approval for the exact outstanding Review hash.
    Approve {
        #[command(flatten)]
        operation: MutationArgs,
        #[arg(long)]
        request_hash: String,
    },
    /// Persist cancellation. Its ACK is not a runtime terminal observation.
    Cancel(MutationArgs),
    /// Explicitly reconcile the same job without renewing a claim or deadline.
    Resume(MutationArgs),
    /// Request an audited export into a new allowed destination.
    Export {
        #[command(flatten)]
        operation: MutationArgs,
        #[arg(long)]
        destination: PathBuf,
    },
}

#[derive(Args)]
struct ConnectionArgs {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    token_file: PathBuf,
}
#[derive(Args)]
struct OperationArgs {
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    operation_id: Uuid,
}
#[derive(Args)]
struct MutationArgs {
    #[command(flatten)]
    operation: OperationArgs,
    #[arg(long)]
    command_id: Option<Uuid>,
}
#[derive(Args)]
struct SubmitArgs {
    #[command(flatten)]
    connection: ConnectionArgs,
    #[arg(long)]
    operation_id: Option<Uuid>,
    #[arg(long, default_value = ".")]
    project: PathBuf,
    #[arg(long)]
    timeout_seconds: Option<u32>,
    #[arg(long)]
    capture_output: bool,
    #[arg(long,value_parser=["codex","claude-code","auto"])]
    client: Option<String>,
    #[arg(long, value_enum)]
    auth_mode: Option<AuthPreference>,
    #[arg(long)]
    client_profile: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(last=true,num_args=0..)]
    argv: Vec<String>,
}

fn command_id(value: Option<Uuid>) -> Result<Uuid> {
    let value = value.unwrap_or_else(Uuid::new_v4);
    ensure!(!value.is_nil(), "operation identities must not be nil");
    eprintln!("Command: {value}");
    Ok(value)
}

async fn control_info(gateway: String, token_file: PathBuf) -> Result<()> {
    let file = PrivateFile::open(&token_file, 4096)
        .map_err(|_| anyhow!("private control credential file is unavailable or unsafe"))?;
    let credential = std::str::from_utf8(file.bytes())
        .map_err(|_| anyhow!("invalid control credential file"))?
        .trim();
    file.revalidate()
        .map_err(|_| anyhow!("control credential file changed"))?;
    let client = AuditClient::new(&gateway, credential)
        .map_err(|_| anyhow!("control gateway origin or credential is invalid"))?;
    let protocol = client.supervision_identity().await?;
    file.revalidate()
        .map_err(|_| anyhow!("control credential file changed"))?;
    let value = serde_json::to_value(&protocol)
        .map_err(|_| anyhow!("control identity cannot be rendered"))?;
    ensure!(
        Redactor::new(vec![credential.into()]).value(&value) == value,
        "control identity contains sensitive data and cannot be displayed"
    );
    let rendered = serde_json::to_string_pretty(&protocol)
        .map_err(|_| anyhow!("control identity cannot be rendered"))?;
    // Redactor's literal dictionary excludes credentials shorter than four
    // bytes; the final output must not expose those credentials either.
    ensure!(
        !rendered.contains(credential),
        "control identity contains sensitive data and cannot be displayed"
    );
    println!("{rendered}");
    Ok(())
}

async fn send(connection: ConnectionArgs, command: BrokerCommandV1) -> Result<()> {
    let recovery_inventory = matches!(&command, BrokerCommandV1::RecoveryInventory { .. });
    let file = PrivateFile::open(&connection.token_file, 4096)
        .map_err(|_| anyhow!("private broker credential file is unavailable or unsafe"))?;
    let credential = std::str::from_utf8(file.bytes())
        .map_err(|_| anyhow!("invalid broker credential file"))?
        .trim();
    file.revalidate()
        .map_err(|_| anyhow!("broker credential file changed"))?;
    let reply =
        service::request(&connection.root, &connection.profile, credential, command).await?;
    if recovery_inventory
        && matches!(
            &reply,
            BrokerReplyV1::Error {
                code: service::protocol::BrokerErrorCodeV1::NeedsReconciliation
            }
        )
    {
        eprintln!(
            "Cold profile discovery requires a broker explicitly started with broker serve --config <config> --recovery-only; inspect other reconciliation errors before retrying."
        );
    }
    if let BrokerReplyV1::Accepted {
        provider_resolution: Some(resolution),
        ..
    } = &reply
    {
        eprintln!(
            "Resolved client: {} / {:?}",
            resolution.provider_name(),
            resolution.auth_mode
        );
    }
    let denied = matches!(
        &reply,
        BrokerReplyV1::Error { .. } | BrokerReplyV1::Rejected { .. }
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&reply)
            .map_err(|_| anyhow!("broker reply cannot be rendered"))?
    );
    ensure!(
        !denied,
        "broker request was not accepted; inspect its response code"
    );
    Ok(())
}

pub async fn execute(args: BrokerArgs) -> Result<()> {
    match args.command {
        BrokerCommand::ControlInfo {
            gateway,
            token_file,
        } => control_info(gateway, token_file).await,
        BrokerCommand::Serve {
            config,
            recovery_only,
        } => {
            if recovery_only {
                service::serve_recovery(&config).await
            } else {
                service::serve(&config).await
            }
        }
        BrokerCommand::Submit(args) => {
            let operation_id = args.operation_id.unwrap_or_else(Uuid::new_v4);
            ensure!(!operation_id.is_nil(), "operation identity must not be nil");
            eprintln!("Operation: {operation_id}");
            let execution = match args.client {
                Some(client) => {
                    ensure!(
                        args.argv.is_empty(),
                        "managed client selection cannot also supply manual argv"
                    );
                    RequestedExecutionV1::Client {
                        client,
                        auth_mode: args.auth_mode,
                        client_profile: args.client_profile,
                        prompt: args
                            .prompt
                            .ok_or_else(|| anyhow!("managed client requires --prompt"))?,
                        model: args.model,
                    }
                }
                None => {
                    ensure!(
                        args.auth_mode.is_none()
                            && args.client_profile.is_none()
                            && args.prompt.is_none()
                            && args.model.is_none(),
                        "client options require --client"
                    );
                    ensure!(
                        !args.argv.is_empty(),
                        "provide a manual command after -- or select --client"
                    );
                    RequestedExecutionV1::Command { argv: args.argv }
                }
            };
            let payload = service::encode_submit(&SubmitV1 {
                version: 1,
                operation_id,
                project: absolute_path(&args.project)
                    .map_err(|_| anyhow!("invalid project path"))?,
                execution,
                timeout_seconds: args.timeout_seconds,
                capture_output: args.capture_output,
            })?;
            send(args.connection, BrokerCommandV1::Submit { payload }).await
        }
        BrokerCommand::Status(args) => {
            send(
                args.connection,
                BrokerCommandV1::Status {
                    operation_id: args.operation_id,
                },
            )
            .await
        }
        BrokerCommand::RecoveryStatus(args) => {
            send(
                args.connection,
                BrokerCommandV1::RecoveryStatus {
                    operation_id: args.operation_id,
                },
            )
            .await
        }
        BrokerCommand::RecoveryInventory {
            connection,
            after,
            limit,
        } => {
            send(
                connection,
                BrokerCommandV1::RecoveryInventory { after, limit },
            )
            .await
        }
        BrokerCommand::List {
            connection,
            after,
            limit,
        } => send(connection, BrokerCommandV1::List { after, limit }).await,
        BrokerCommand::Approve {
            operation,
            request_hash,
        } => {
            let command_id = command_id(operation.command_id)?;
            send(
                operation.operation.connection,
                BrokerCommandV1::Approve {
                    operation_id: operation.operation.operation_id,
                    command_id,
                    request_hash,
                },
            )
            .await
        }
        BrokerCommand::Cancel(operation) => {
            let command_id = command_id(operation.command_id)?;
            send(
                operation.operation.connection,
                BrokerCommandV1::Cancel {
                    operation_id: operation.operation.operation_id,
                    command_id,
                },
            )
            .await
        }
        BrokerCommand::Resume(operation) => {
            let command_id = command_id(operation.command_id)?;
            send(
                operation.operation.connection,
                BrokerCommandV1::Resume {
                    operation_id: operation.operation.operation_id,
                    command_id,
                },
            )
            .await
        }
        BrokerCommand::Export {
            operation,
            destination,
        } => {
            let command_id = command_id(operation.command_id)?;
            let destination =
                absolute_path(&destination).map_err(|_| anyhow!("invalid export path"))?;
            send(
                operation.operation.connection,
                BrokerCommandV1::Export {
                    operation_id: operation.operation.operation_id,
                    command_id,
                    destination,
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: BrokerArgs,
    }

    #[test]
    fn broker_recovery_inventory_is_available_without_a_known_operation_uuid() {
        let discovered = TestCli::try_parse_from([
            "broker",
            "recovery-inventory",
            "--root",
            "/private/fixture",
            "--profile",
            "fixture",
            "--token-file",
            "/private/token",
            "--limit",
            "1",
        ]);
        assert!(
            discovered.is_ok(),
            "discovery must be available before its UUID is known"
        );
        assert!(
            TestCli::try_parse_from([
                "broker",
                "recovery-status",
                "--root",
                "/private/fixture",
                "--profile",
                "fixture",
                "--token-file",
                "/private/token",
                "--operation-id",
                "11111111-1111-4111-8111-111111111111",
            ])
            .is_ok(),
            "positive control: known-UUID recovery status exists already"
        );
        assert!(
            TestCli::try_parse_from([
                "broker",
                "recovery-status",
                "--root",
                "/private/fixture",
                "--profile",
                "fixture",
                "--token-file",
                "/private/token",
            ])
            .is_err(),
            "existing status still requires an explicit operation UUID"
        );
    }

    #[test]
    fn broker_recovery_mode_is_selected_explicitly() {
        let recovery = TestCli::try_parse_from([
            "broker",
            "serve",
            "--config",
            "/private/fixture.json",
            "--recovery-only",
        ])
        .unwrap();
        assert!(matches!(
            recovery.args.command,
            BrokerCommand::Serve {
                recovery_only: true,
                ..
            }
        ));
        let legacy =
            TestCli::try_parse_from(["broker", "serve", "--config", "/private/fixture.json"])
                .unwrap();
        assert!(matches!(
            legacy.args.command,
            BrokerCommand::Serve {
                recovery_only: false,
                ..
            }
        ));
    }
}
