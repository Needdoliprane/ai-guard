use agentsandbox::{
    broker::service::{
        self, BrokerClient, BrokerClientSelection, RequestedExecutionV1,
        ServiceEnforcerSelectionV1, ServicePlatform, SubmitV1,
        protocol::{BrokerCommandV1, BrokerReplyV1, JOB_RESULT_VERSION},
    },
    control_fs::{PrivateFile, absolute_path},
    provider::AuthPreference,
    redaction::Redactor,
    runner::AuditClient,
};
use anyhow::{Result, anyhow, ensure};
use clap::{ArgGroup, Args, Subcommand};
use std::path::PathBuf;
use uuid::Uuid;

mod policy;

#[derive(Args)]
pub struct BrokerArgs {
    #[command(subcommand)]
    command: BrokerCommand,
}

#[derive(Subcommand)]
enum BrokerCommand {
    /// Prepare and explicitly submit retained owned policy registry operations.
    Policy {
        #[command(subcommand)]
        command: policy::PolicyCommand,
    },
    /// Read authenticated control metadata without starting or activating a broker.
    ControlInfo {
        #[arg(long)]
        gateway: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Publish an exact broker profile selection in a new private directory.
    PrepareClient {
        /// Absolute path to the private broker registry.
        #[arg(long)]
        broker_config: PathBuf,
        /// Operator-configured broker profile to retain.
        #[arg(long)]
        profile: String,
        /// New absolute directory for the selection.
        #[arg(long)]
        destination: PathBuf,
    },
    /// Plan the foreground broker service for the local system manager.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Run the explicitly configured trusted local broker in the foreground.
    Serve {
        #[arg(long)]
        config: PathBuf,
        /// Restore only proven STOP targets; never open admission or initialize auth.
        #[arg(long)]
        recovery_only: bool,
        /// Create a fresh Full3 enforcer in a new private role child.
        #[arg(
            long,
            value_name = "ROLE",
            value_parser = parse_enforcer_role,
            conflicts_with_all = ["enforcer_full3_existing", "recovery_only", "enforcer_role"]
        )]
        enforcer_full3_new: Option<String>,
        /// Connect explicitly to an existing sealed Full3 role without spawning it.
        #[arg(
            long,
            value_name = "ROLE",
            value_parser = parse_enforcer_role,
            conflicts_with_all = ["enforcer_full3_new", "recovery_only", "enforcer_role"]
        )]
        enforcer_full3_existing: Option<String>,
        /// Restore proven STOP targets from an existing sealed private role.
        #[arg(
            long,
            value_name = "ROLE",
            value_parser = parse_enforcer_role,
            requires = "recovery_only",
            conflicts_with_all = ["enforcer_full3_new", "enforcer_full3_existing"]
        )]
        enforcer_role: Option<String>,
    },
    /// Persist a new batch job. The broker continues after this client exits.
    Submit(SubmitArgs),
    /// Read cached journal/control facts for this identity's job.
    Status(OperationArgs),
    /// Read the durable batch result for this identity's operation.
    Result(OperationArgs),
    /// Read the retained relay attachment state and its coverage for this job.
    RelayStatus(OperationArgs),
    /// Read confirmed recovery metadata, including when business loading fails.
    /// Does not inspect the runtime or establish that a worker has stopped.
    RecoveryStatus(OperationArgs),
    /// Read explicit D5 preclaim/retirement metadata; requires V2 support.
    /// Never falls back to the historical diagnostic on an older service.
    RecoveryStatusV2(OperationArgs),
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
    /// Inhibit one original Full3 Arm before its claim. ACK means audited request.
    AbandonBeforeClaim {
        #[command(flatten)]
        operation: OperationArgs,
        #[arg(long)]
        arm_id: Uuid,
        #[arg(long)]
        expected_epoch: Uuid,
        #[arg(long)]
        generation: u64,
        #[arg(long)]
        command_id: Uuid,
    },
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

#[derive(Subcommand)]
enum ServiceCommand {
    /// Render a proposed service file and diff on stdout.
    Plan {
        #[arg(long, value_enum)]
        platform: ServicePlatform,
        /// Absolute path to the existing private broker registry.
        #[arg(long)]
        config: PathBuf,
        /// Existing Full3 role whose guardian must be supervised independently.
        #[arg(long, value_name = "ROLE", value_parser = parse_enforcer_role)]
        enforcer_full3_existing: String,
    },
}

#[derive(Args)]
#[command(group(ArgGroup::new("broker_connection")
    .args(["root", "broker_config", "broker_selection"])
    .required(true)))]
struct ConnectionArgs {
    /// Use the already-running broker at this private root.
    #[arg(long, requires_all = ["profile", "token_file"])]
    root: Option<PathBuf>,
    /// Resolve the broker root and credential from this absolute private broker registry path.
    #[arg(long, requires = "profile", conflicts_with = "token_file")]
    broker_config: Option<PathBuf>,
    /// Use a private broker selection published by broker prepare-client.
    #[arg(long, env = "AGENTSANDBOX_BROKER_SELECTION", hide_env_values = true,
        conflicts_with_all = ["profile", "token_file"])]
    broker_selection: Option<PathBuf>,
    /// Select an operator-configured broker profile.
    #[arg(long)]
    profile: Option<String>,
    /// Private credential file for the selected profile when providing --root.
    #[arg(long, requires = "root")]
    token_file: Option<PathBuf>,
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

fn parse_enforcer_role(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || matches!(value, "broker" | "journals" | "enforcer")
    {
        return Err("enforcer role must be a non-reserved private child name of 1–100 ASCII letters, digits, hyphens or underscores".into());
    }
    Ok(value.into())
}

fn serve_enforcer_selection(
    recovery_only: bool,
    full3_new: Option<String>,
    full3_existing: Option<String>,
    enforcer_role: Option<String>,
) -> Result<ServiceEnforcerSelectionV1> {
    match (recovery_only, full3_new, full3_existing, enforcer_role) {
        (false, None, None, None) => Ok(ServiceEnforcerSelectionV1::Legacy),
        (true, None, None, role_name) => Ok(ServiceEnforcerSelectionV1::Recovery { role_name }),
        (false, Some(role_name), None, None) => {
            Ok(ServiceEnforcerSelectionV1::Full3Fresh { role_name })
        }
        (false, None, Some(role_name), None) => {
            Ok(ServiceEnforcerSelectionV1::Full3Existing { role_name })
        }
        _ => Err(anyhow!("incompatible enforcer selection")),
    }
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
    let result_request = match &command {
        BrokerCommandV1::Result {
            version,
            operation_id,
        } => Some((*version, *operation_id)),
        _ => None,
    };
    let client = match (
        connection.broker_selection,
        connection.broker_config,
        connection.root,
        connection.profile,
        connection.token_file,
    ) {
        (Some(selection), None, None, None, None) => {
            BrokerClientSelection::open(&selection)?.into_client()?
        }
        (None, Some(config), None, Some(profile), None) => {
            BrokerClient::open_from_config(&config, profile)?
        }
        (None, None, Some(root), Some(profile), Some(token_file)) => {
            BrokerClient::open(root, profile, &token_file)?
        }
        _ => return Err(anyhow!("invalid broker selection")),
    };
    let reply = client.request(command).await?;
    if let Some((version, operation_id)) = result_request {
        ensure!(
            matches!(&reply, BrokerReplyV1::Result { result }
                if result.version == version && result.operation_id == operation_id)
                || matches!(&reply, BrokerReplyV1::Error { .. }),
            "incompatible or uncorrelated broker result reply"
        );
    }
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
        !matches!(&reply, BrokerReplyV1::AbandonedBeforeClaim { .. }),
        "execution was abandoned before its claim; the requested operation did not complete"
    );
    ensure!(
        !denied,
        "broker request was not accepted; inspect its response code"
    );
    Ok(())
}

pub async fn execute(args: BrokerArgs) -> Result<()> {
    match args.command {
        BrokerCommand::Policy { command } => policy::execute(command).await,
        BrokerCommand::ControlInfo {
            gateway,
            token_file,
        } => control_info(gateway, token_file).await,
        BrokerCommand::PrepareClient {
            broker_config,
            profile,
            destination,
        } => BrokerClientSelection::publish(&destination, &broker_config, &profile),
        BrokerCommand::Service {
            command:
                ServiceCommand::Plan {
                    platform,
                    config,
                    enforcer_full3_existing,
                },
        } => {
            let plan = service::plan_service_unit(platform, &config, &enforcer_full3_existing)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
            Ok(())
        }
        BrokerCommand::Serve {
            config,
            recovery_only,
            enforcer_full3_new,
            enforcer_full3_existing,
            enforcer_role,
        } => {
            let selection = serve_enforcer_selection(
                recovery_only,
                enforcer_full3_new,
                enforcer_full3_existing,
                enforcer_role,
            )?;
            match service::serve_selected_owned(&config, selection).await? {
                service::ServiceShutdown::Drained => Ok(()),
                service::ServiceShutdown::Retained(owner) => {
                    // Keep the actual D5/backends through recovery supervision.
                    // Never erase this outcome with a generic error conversion.
                    owner.supervise().await
                }
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
        BrokerCommand::Result(args) => {
            send(
                args.connection,
                BrokerCommandV1::Result {
                    version: JOB_RESULT_VERSION,
                    operation_id: args.operation_id,
                },
            )
            .await
        }
        BrokerCommand::RelayStatus(args) => {
            send(
                args.connection,
                BrokerCommandV1::RelayStatus {
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
        BrokerCommand::RecoveryStatusV2(args) => {
            send(
                args.connection,
                BrokerCommandV1::RecoveryStatusV2 {
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
        BrokerCommand::AbandonBeforeClaim {
            operation,
            arm_id,
            expected_epoch,
            generation,
            command_id,
        } => {
            let request = service::AbandonBeforeClaimV1 {
                version: 1,
                operation_id: operation.operation_id,
                arm_id,
                expected_epoch,
                generation,
                command_id,
            };
            request.validate()?;
            send(
                operation.connection,
                BrokerCommandV1::AbandonBeforeClaim { request },
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
    fn broker_service_plan_requires_a_platform_and_an_existing_full3_role() {
        for platform in ["systemd", "launchd"] {
            let command = TestCli::try_parse_from([
                "broker",
                "service",
                "plan",
                "--platform",
                platform,
                "--config",
                "/private/fixture.json",
                "--enforcer-full3-existing",
                "full3-qa",
            ])
            .unwrap();
            assert!(matches!(command.args.command,
                BrokerCommand::Service {
                    command: ServiceCommand::Plan { enforcer_full3_existing, .. }
                } if enforcer_full3_existing == "full3-qa"));
        }
        for args in [
            &["--platform", "systemd"][..],
            &["--enforcer-full3-existing", "full3-qa"][..],
            &[
                "--platform",
                "systemd",
                "--enforcer-full3-existing",
                "enforcer",
            ][..],
            &["--platform", "systemd", "--enforcer-full3-new", "full3-qa"][..],
        ] {
            let mut argv = vec![
                "broker",
                "service",
                "plan",
                "--config",
                "/private/fixture.json",
            ];
            argv.extend_from_slice(args);
            assert!(TestCli::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn broker_service_plan_argv_reparses_the_exact_existing_role() {
        // The plan renders the role as one argv element to preserve a leading
        // hyphen. TestCli flattens the outer broker subcommand.
        let argv = [
            "/opt/ai guard/agentsandbox",
            "broker",
            "serve",
            "--config",
            "/private/ai guard/registry.json",
            "--enforcer-full3-existing=-full3-qa",
        ];
        let parsed =
            TestCli::try_parse_from(std::iter::once(argv[0]).chain(argv[2..].iter().copied()))
                .unwrap();
        let BrokerCommand::Serve {
            config,
            recovery_only,
            enforcer_full3_new,
            enforcer_full3_existing,
            enforcer_role,
        } = parsed.args.command
        else {
            panic!("planned argv did not select broker serve");
        };
        assert_eq!(config, PathBuf::from(argv[4]));
        assert!(matches!(
            serve_enforcer_selection(recovery_only, enforcer_full3_new, enforcer_full3_existing, enforcer_role).unwrap(),
            ServiceEnforcerSelectionV1::Full3Existing { role_name } if role_name == "-full3-qa"
        ));
    }

    #[test]
    fn broker_recovery_status_versions_are_selected_explicitly() {
        for (name, extended) in [("recovery-status", false), ("recovery-status-v2", true)] {
            let parsed = TestCli::try_parse_from([
                "broker",
                name,
                "--root",
                "/private/fixture",
                "--profile",
                "fixture",
                "--token-file",
                "/private/token",
                "--operation-id",
                "11111111-1111-4111-8111-111111111111",
            ])
            .unwrap();
            match parsed.args.command {
                BrokerCommand::RecoveryStatus(_) => assert!(!extended),
                BrokerCommand::RecoveryStatusV2(_) => assert!(extended),
                _ => panic!("explicit recovery diagnostic selected another command"),
            }
        }
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

    // Parser and selection checks do not start a service or prove that its
    // selected enforcer is available, authenticated, or eligible for admission.
    fn selected_enforcer(options: &[&str]) -> ServiceEnforcerSelectionV1 {
        let mut argv = vec!["broker", "serve", "--config", "/private/fixture.json"];
        argv.extend_from_slice(options);
        let args = TestCli::try_parse_from(argv).unwrap();
        let BrokerCommand::Serve {
            recovery_only,
            enforcer_full3_new,
            enforcer_full3_existing,
            enforcer_role,
            ..
        } = args.args.command
        else {
            panic!("expected broker serve");
        };
        serve_enforcer_selection(
            recovery_only,
            enforcer_full3_new,
            enforcer_full3_existing,
            enforcer_role,
        )
        .unwrap()
    }

    #[test]
    fn broker_serve_preserves_default_legacy_and_explicit_recovery() {
        assert!(matches!(
            selected_enforcer(&[]),
            ServiceEnforcerSelectionV1::Legacy
        ));
        assert!(matches!(
            selected_enforcer(&["--recovery-only"]),
            ServiceEnforcerSelectionV1::Recovery { role_name: None }
        ));
        assert!(matches!(
            selected_enforcer(&["--recovery-only", "--enforcer-role", "stops_3-qa"]),
            ServiceEnforcerSelectionV1::Recovery { role_name: Some(name) }
                if name == "stops_3-qa"
        ));
    }

    #[test]
    fn broker_serve_selects_full3_fresh_or_existing_explicitly() {
        assert!(matches!(
            selected_enforcer(&["--enforcer-full3-new", "full3_qa-1"]),
            ServiceEnforcerSelectionV1::Full3Fresh { role_name }
                if role_name == "full3_qa-1"
        ));
        assert!(matches!(
            selected_enforcer(&["--enforcer-full3-existing", "full3_qa-1"]),
            ServiceEnforcerSelectionV1::Full3Existing { role_name }
                if role_name == "full3_qa-1"
        ));
        let maximum_name = "A".repeat(100);
        assert!(matches!(
            selected_enforcer(&["--enforcer-full3-new", &maximum_name]),
            ServiceEnforcerSelectionV1::Full3Fresh { role_name }
                if role_name == maximum_name
        ));
    }

    #[test]
    fn broker_serve_rejects_incompatible_enforcer_selections() {
        let incompatible = [
            vec![
                "--enforcer-full3-new",
                "full3",
                "--enforcer-full3-existing",
                "full3",
            ],
            vec!["--enforcer-full3-new", "full3", "--recovery-only"],
            vec!["--enforcer-full3-existing", "full3", "--recovery-only"],
            vec!["--enforcer-role", "full3"],
            vec!["--enforcer-role", "full3", "--enforcer-full3-new", "full3"],
            vec![
                "--enforcer-role",
                "full3",
                "--enforcer-full3-existing",
                "full3",
            ],
        ];
        for options in incompatible {
            let mut argv = vec!["broker", "serve", "--config", "/private/fixture.json"];
            argv.extend(options);
            assert!(TestCli::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn broker_serve_rejects_role_paths_reserved_names_and_invalid_components() {
        let overlong = "a".repeat(101);
        let invalid_names = [
            "",
            ".",
            "..",
            "../other",
            "role/child",
            "/private/role",
            "role\\child",
            "role.name",
            "role name",
            "rôle",
            "role\0name",
            "broker",
            "journals",
            "enforcer",
            &overlong,
        ];
        for option in [
            "--enforcer-full3-new",
            "--enforcer-full3-existing",
            "--enforcer-role",
        ] {
            for name in invalid_names {
                let mut argv = vec!["broker", "serve", "--config", "/private/fixture.json"];
                if option == "--enforcer-role" {
                    argv.push("--recovery-only");
                }
                argv.extend([option, name]);
                assert!(TestCli::try_parse_from(argv).is_err());
            }
        }
    }
}

#[cfg(test)]
mod policy_baseline_tests;
