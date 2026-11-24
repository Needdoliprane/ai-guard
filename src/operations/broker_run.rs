use super::require_successful_run;
use crate::{
    broker::service::{
        BrokerClient, BrokerClientSelection, RequestedExecutionV1, SubmitV1, encode_submit,
        protocol::{
            BrokerCommandV1, BrokerErrorCodeV1, BrokerReplyV1, JOB_RESULT_VERSION,
            JobResultOutcomeV1,
        },
    },
    control_fs::absolute_path,
    runner::{self, RunOptions},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::{ArgGroup, Args};
use std::{io::Write, path::PathBuf, time::Duration};
use tokio::{
    signal::unix::{SignalKind, signal},
    time::{Instant, MissedTickBehavior, interval, sleep_until, timeout_at},
};
use uuid::Uuid;

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("broker_source")
    .args(["broker_root", "broker_config", "broker_selection"])
    .conflicts_with_all([
        "config_dir", "gateway", "runtime", "runtime_program", "runtime_endpoint", "image",
        "profile", "network", "allow_rootful_runtime", "provider", "providers_file", "approve_hash",
        "interactive", "secret_env_file"
    ])))]
pub struct RunCommand {
    #[command(flatten)]
    run: RunOptions,
    /// Use the already-running broker at this private root.
    #[arg(long, requires_all = ["broker_profile", "broker_token_file"])]
    broker_root: Option<PathBuf>,
    /// Resolve the broker root and credential from this absolute private broker registry path.
    #[arg(
        long,
        requires = "broker_profile",
        conflicts_with = "broker_token_file"
    )]
    broker_config: Option<PathBuf>,
    /// Use a private broker selection published by broker prepare-client.
    #[arg(long, env = "AGENTSANDBOX_BROKER_SELECTION", hide_env_values = true,
        conflicts_with_all = ["broker_profile", "broker_token_file"])]
    broker_selection: Option<PathBuf>,
    /// Select an operator-configured broker profile.
    #[arg(long, requires = "broker_source")]
    broker_profile: Option<String>,
    /// Private credential file for the selected broker profile.
    #[arg(long, requires = "broker_root")]
    broker_token_file: Option<PathBuf>,
    /// Reuse this operation ID with identical inputs after a lost reply.
    #[arg(long, requires = "broker_source")]
    operation_id: Option<Uuid>,
    /// Limit client waiting for a result after acceptance, in seconds (default 600).
    #[arg(long, requires = "broker_source", value_parser = clap::value_parser!(u64).range(1..=86_400))]
    broker_wait_seconds: Option<u64>,
    #[arg(skip)]
    explicit_timeout: bool,
}

impl RunCommand {
    pub fn set_explicit_timeout(&mut self, explicit: bool) {
        self.explicit_timeout = explicit;
    }

    pub(super) async fn execute(self) -> Result<()> {
        if self.broker_root.is_none()
            && self.broker_config.is_none()
            && self.broker_selection.is_none()
        {
            let summary = match Box::pin(runner::run(self.run)).await {
                Ok(summary) => summary,
                Err(error) => {
                    if let Some(summary) = runner::confirmed_deadline_summary(&error) {
                        if let Ok(json) = serde_json::to_string_pretty(summary) {
                            // Reporting is best-effort here. A closed stdout must preserve
                            // the original execution failure.
                            let _ = writeln!(std::io::stdout().lock(), "{json}");
                        }
                    }
                    return Err(error);
                }
            };
            println!("{}", serde_json::to_string_pretty(&summary)?);
            return require_successful_run(&summary);
        }

        let operation_id = self.operation_id.unwrap_or_else(Uuid::new_v4);
        ensure!(
            !operation_id.is_nil(),
            "operation identities must not be nil"
        );
        let timeout_seconds = if self.explicit_timeout {
            ensure!(
                (1..=86_400).contains(&self.run.timeout),
                "broker run duration must be between 1 and 86400 seconds"
            );
            Some(u32::try_from(self.run.timeout).context("invalid broker run duration")?)
        } else {
            None
        };
        let project =
            absolute_path(&self.run.project).map_err(|_| anyhow!("invalid project path"))?;
        let capture_output = self.run.capture_output;
        let payload = encode_submit(&SubmitV1 {
            version: 1,
            operation_id,
            project,
            execution: execution(self.run)?,
            timeout_seconds,
            capture_output,
        })?;
        eprintln!("Operation: {operation_id}");
        let client = match (
            self.broker_selection,
            self.broker_config,
            self.broker_root,
            self.broker_profile,
            self.broker_token_file,
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
            _ => bail!("invalid broker selection"),
        };
        let mut interrupt =
            signal(SignalKind::interrupt()).context("cannot install broker interrupt handler")?;
        match client.request(BrokerCommandV1::Submit { payload }).await? {
            BrokerReplyV1::Accepted {
                operation_id: accepted,
                provider_resolution,
                ..
            } if accepted == operation_id => {
                if let Some(resolution) = provider_resolution {
                    eprintln!(
                        "Resolved client: {} / {:?}",
                        resolution.provider_name(),
                        resolution.auth_mode
                    );
                }
            }
            reply @ BrokerReplyV1::Rejected {
                operation_id: rejected,
                ..
            } if rejected == operation_id => {
                println!("{}", serde_json::to_string_pretty(&reply)?);
                bail!("broker rejected the submission");
            }
            BrokerReplyV1::Error { code } => bail!("broker submission failed: {code}"),
            _ => bail!("unexpected broker submission reply; inspect the original operation"),
        }
        let deadline =
            Instant::now() + Duration::from_secs(self.broker_wait_seconds.unwrap_or(600));
        let mut ticks = interval(Duration::from_secs(1));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut cancellation_sent = false;
        let mut reported_unavailability = false;
        loop {
            let reply = tokio::select! {
                biased;
                _ = sleep_until(deadline) => bail!("broker result wait expired; inspect the original operation"),
                _ = interrupt.recv() => {
                    ensure!(!cancellation_sent, "wait interrupted; inspect the original operation and cancellation");
                    let command_id = Uuid::new_v4();
                    eprintln!("Command: {command_id}");
                    let command = BrokerCommandV1::Cancel { operation_id, command_id };
                    let reply = timeout_at(deadline, client.request(command)).await
                        .context("broker cancellation outcome is unknown; retry the same command identity")?
                        .context("broker cancellation outcome is unknown; retry the same command identity")?;
                    match reply {
                        BrokerReplyV1::CommandAcknowledged { operation_id: acknowledged_op,
                            command_id: acknowledged_command, .. }
                            if acknowledged_op == operation_id && acknowledged_command == command_id =>
                        {
                            cancellation_sent = true;
                        }
                        BrokerReplyV1::Error { code } => bail!("broker cancellation failed: {code}"),
                        _ => bail!("unexpected cancellation acknowledgement; inspect the original operation"),
                    }
                    continue;
                }
                reply = async {
                    ticks.tick().await;
                    client.request(BrokerCommandV1::Result { version: JOB_RESULT_VERSION, operation_id }).await
                } => reply.context("broker result unavailable; inspect the original operation")?,
            };
            let result = match reply {
                BrokerReplyV1::Result { result }
                    if result.version == JOB_RESULT_VERSION
                        && result.operation_id == operation_id =>
                {
                    result
                }
                BrokerReplyV1::Error {
                    code: BrokerErrorCodeV1::Unavailable,
                } => {
                    if !reported_unavailability {
                        eprintln!(
                            "Broker result is unavailable. Waiting for the original operation."
                        );
                        reported_unavailability = true;
                    }
                    continue;
                }
                BrokerReplyV1::Error { code } => bail!("broker result unavailable: {code}"),
                _ => bail!("unexpected broker result; inspect the original operation"),
            };
            let exit = match &result.outcome {
                JobResultOutcomeV1::Pending => continue,
                JobResultOutcomeV1::Terminal { result } if result.oom_killed == Some(true) => Err(
                    anyhow!("worker runtime reported an out-of-memory termination"),
                ),
                JobResultOutcomeV1::Terminal { result } if result.exit_code == 0 => Ok(()),
                JobResultOutcomeV1::Terminal { result } => {
                    Err(anyhow!("worker exited with code {}", result.exit_code))
                }
                JobResultOutcomeV1::Review { .. } => {
                    Err(anyhow!("broker job requires approval via broker approve"))
                }
                JobResultOutcomeV1::Refused { .. } => Err(anyhow!("broker job was refused")),
                JobResultOutcomeV1::Cancelled => Err(anyhow!("broker job was cancelled")),
                JobResultOutcomeV1::Ambiguous { reason } => {
                    Err(anyhow!("broker needs reconciliation: {reason}"))
                }
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            return exit;
        }
    }
}

fn execution(options: RunOptions) -> Result<RequestedExecutionV1> {
    if let Some(client) = options.client {
        ensure!(
            options.argv.is_empty(),
            "managed client selection cannot also supply manual argv"
        );
        return Ok(RequestedExecutionV1::Client {
            client,
            auth_mode: options.auth_mode,
            client_profile: options.client_profile,
            prompt: options.prompt.context("managed client requires --prompt")?,
            model: options.model,
        });
    }
    ensure!(
        options.auth_mode.is_none()
            && options.client_profile.is_none()
            && options.prompt.is_none()
            && options.model.is_none(),
        "client options require --client"
    );
    ensure!(
        !options.argv.is_empty(),
        "provide a manual command after -- or select --client"
    );
    Ok(RequestedExecutionV1::Command { argv: options.argv })
}
