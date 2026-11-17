use crate::{
    runner::{self, RunOptions},
    runtime::RuntimeKind,
};
use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

mod source_snapshot;

#[derive(Subcommand, Debug)]
pub enum OperatorCommand {
    /// Create isolated configuration and two distinct random authentication keys.
    Init {
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
    },
    /// Prepare or inspect explicit private provider profiles, without model calls.
    Providers {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Inspect the installed engine; never falls back to local command execution.
    Doctor {
        #[arg(long, value_enum, default_value = "docker")]
        runtime: RuntimeKind,
        /// Absolute engine program approved by the control operator.
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        /// Explicit Docker Unix endpoint; otherwise resolve the control account's context once.
        #[arg(long)]
        runtime_endpoint: Option<String>,
        #[arg(long)]
        allow_rootful_runtime: bool,
    },
    /// Build the control plane and worker images with the selected engine.
    Build {
        #[arg(long, value_enum, default_value = "docker")]
        runtime: RuntimeKind,
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        #[arg(long)]
        runtime_endpoint: Option<String>,
        #[arg(long, default_value = ".")]
        source: PathBuf,
        #[arg(long)]
        allow_rootful_runtime: bool,
        #[arg(long)]
        codex_version: Option<String>,
        #[arg(long)]
        claude_code_version: Option<String>,
        /// Optional descriptive source revision; never resolved from ambient Git.
        #[arg(long)]
        build_revision: Option<String>,
    },
    /// Start the gateway and the separately containerized dashboard.
    Up {
        #[arg(long, value_enum, default_value = "docker")]
        runtime: RuntimeKind,
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        #[arg(long)]
        runtime_endpoint: Option<String>,
        #[arg(long, default_value = ".")]
        source: PathBuf,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long)]
        allow_rootful_runtime: bool,
    },
    /// Stop only the control-plane stack, retaining audit and session volumes.
    Down {
        #[arg(long, value_enum, default_value = "docker")]
        runtime: RuntimeKind,
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        #[arg(long)]
        runtime_endpoint: Option<String>,
        #[arg(long, default_value = ".")]
        source: PathBuf,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
    },
    /// Ask policy permission, import a filtered copy, and execute inside a worker.
    Run(Box<RunOptions>),
    /// Read this session's worker from its authenticated tenant registry.
    WorkerStatus {
        #[arg(long)]
        session: uuid::Uuid,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        gateway: String,
    },
    /// Reconcile an interrupted worker without replaying a consumed start.
    WorkerResume {
        /// First-start an admitted interactive session; consumed claims cannot reattach yet.
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        /// Explicitly bind a legacy session to this approved engine program.
        #[arg(long)]
        adopt_runtime_program: bool,
        #[arg(long)]
        session: uuid::Uuid,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        gateway: String,
        #[arg(long, default_value_t = 30)]
        wait_seconds: u64,
    },
    /// Persist a stop intent, then stop only the owned runtime instance.
    WorkerStop {
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        #[arg(long)]
        adopt_runtime_program: bool,
        #[arg(long)]
        session: uuid::Uuid,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        gateway: String,
    },
    /// Safely export a retained session volume into a new directory.
    ExportWorkspace {
        #[arg(long)]
        runtime_program: Option<PathBuf>,
        #[arg(long)]
        adopt_runtime_program: bool,
        #[arg(long)]
        session: uuid::Uuid,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        gateway: String,
    },
    /// Ask the gateway to verify its local hash chain.
    VerifyAudit {
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        /// Private viewer credential file. Defaults to <config-dir>/secrets/viewer.
        #[arg(long, value_name = "FILE")]
        viewer_token_file: Option<PathBuf>,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        gateway: String,
        /// Also verify the signed audit checkpoints of this private directory
        /// against --anchor, and print that report after the gateway answer.
        #[arg(long, requires = "anchor")]
        checkpoint_dir: Option<PathBuf>,
        /// Trust anchor held outside the controlled storage.
        #[arg(long, requires = "checkpoint_dir")]
        anchor: Option<PathBuf>,
        /// Audit SQLite database to compare with every trusted signed position.
        #[arg(long, requires = "checkpoint_dir")]
        data: Option<PathBuf>,
    },
    /// Write or verify the release manifest of a build source prepared as `build` prepares it.
    /// Starts no engine and opens no network connection; nothing is signed or uploaded.
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProviderCommand {
    /// Create API profile references; the credential file is never read by init.
    Init {
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        /// Explicit NAME=value file. Only the selected provider key is handed off.
        #[arg(long)]
        env_file: PathBuf,
    },
    /// Show configured profiles without private paths or credential values.
    Status {
        #[arg(long, default_value = ".agentsandbox")]
        config_dir: PathBuf,
        #[arg(long)]
        providers_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

// Every value is checked by the release command itself, so that a malformed
// one is refused with a named reason (exit status 1), not as a usage error.
#[derive(Subcommand, Debug)]
pub enum ReleaseCommand {
    /// Prepare --source as `build` does, then write release-manifest.json, sbom.cdx.json and
    /// SHA256SUMS into a new directory whose parent is an existing private directory.
    Manifest {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Built file to bind by name, size and SHA-256; repeat for each file.
        #[arg(long = "artifact", value_name = "FILE")]
        artifacts: Vec<PathBuf>,
        #[arg(long)]
        codex_version: Option<String>,
        #[arg(long)]
        claude_code_version: Option<String>,
        /// Operator-declared digest (64 lowercase hex) from a `build` provenance line; the
        /// command refuses before any output if the new capture differs. Not authenticated.
        #[arg(long, value_name = "HEX64")]
        expect_source_digest: Option<String>,
    },
    /// Prepare --source again and compare source, locks, inventory and the given artefacts with
    /// a manifest directory; any divergence is refused; a manifest this CLI cannot judge is
    /// UNVERIFIED.
    Verify {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        /// Built file listed in the manifest; repeat for each listed file.
        #[arg(long = "artifact", value_name = "FILE")]
        artifacts: Vec<PathBuf>,
        /// Compare the source, locks and inventory only; no artefact is verified.
        #[arg(long)]
        source_only: bool,
    },
}

fn initialize_provider_references(config_dir: &Path, env_file: &Path) -> Result<()> {
    use crate::provider::{
        AuthMode, ClientKind, CredentialSourceV1, ProviderConfigV1, ProviderProfileV1,
    };
    let config = initialize_config_directory(config_dir)?;
    let env_file = crate::control_fs::absolute_path(env_file)?;
    let providers = ProviderConfigV1 {
        version: 1,
        default_profile: None,
        auto_priority: vec!["codex-api".into(), "claude-api".into()],
        profiles: vec![
            ProviderProfileV1 {
                id: "codex-api".into(),
                revision: 1,
                client: ClientKind::Codex,
                auth_mode: AuthMode::Api,
                model: None,
                credential: CredentialSourceV1::ApiEnvFile {
                    path: env_file.clone(),
                    key: Some("OPENAI_API_KEY".into()),
                },
            },
            ProviderProfileV1 {
                id: "claude-api".into(),
                revision: 1,
                client: ClientKind::ClaudeCode,
                auth_mode: AuthMode::Api,
                model: None,
                credential: CredentialSourceV1::ApiEnvFile {
                    path: env_file,
                    key: Some("ANTHROPIC_API_KEY".into()),
                },
            },
        ],
    };
    providers.validate()?;
    config.revalidate()?;
    let mut file = config.create_private_entry(std::ffi::OsStr::new("providers.json"))
        .context("provider configuration already exists or cannot be created; existing references were preserved")?;
    serde_json::to_writer_pretty(&mut file, &providers)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    config.sync()?;
    Ok(())
}

fn provider_status(config_dir: &Path, providers_file: Option<&Path>) -> Result<serde_json::Value> {
    use crate::provider::CredentialSourceV1;
    let config =
        crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(config_dir)?)
            .context("private configuration missing or unsafe; run agentsandbox init first")?;
    let path = providers_file
        .map(crate::control_fs::absolute_path)
        .transpose()?
        .unwrap_or_else(|| config.path().join("providers.json"));
    let providers = crate::provider::load_config(&path).context(
        "provider configuration missing or unsafe; run agentsandbox providers init --env-file PATH",
    )?;
    let profiles: Vec<_> = providers
        .profiles
        .iter()
        .map(|profile| {
            serde_json::json!({
                "id": profile.id, "revision": profile.revision, "client": profile.client,
                "auth_mode": profile.auth_mode, "model": profile.model,
                "credential_source": match &profile.credential {
                    CredentialSourceV1::ApiEnvFile { .. } => "api_env_file",
                    CredentialSourceV1::CodexAuthFile { .. } => "codex_auth_file",
                    CredentialSourceV1::ClaudeOAuthEnvFile { .. } => "claude_oauth_env_file",
                }
            })
        })
        .collect();
    Ok(serde_json::json!({
        "version": 1, "status": "configured", "profiles": profiles,
        "default_profile": providers.default_profile, "auto_priority": providers.auto_priority,
        "credentials_verified": false, "worker_capabilities_verified": false,
        "mediation": crate::runner::launch_mediation_metadata(),
        "next_action": "Select a profile explicitly for run. Credentials are validated before authorization; image client capabilities and real authentication require qualification."
    }))
}

fn require_successful_run(summary: &runner::RunSummary) -> Result<()> {
    if summary.cleanup.state == runner::CleanupState::Unconfirmed {
        bail!(
            "worker result recorded (exit {}); cleanup unconfirmed for container {}",
            summary.exit_code,
            summary.cleanup.container
        );
    }
    if let Some(reason) = &summary.transport_stop_reason {
        bail!("worker terminal result recorded after transport stop: {reason}");
    }
    if summary.exit_code != 0 || summary.timed_out {
        bail!(
            "worker command failed (exit {}, timeout {})",
            summary.exit_code,
            summary.timed_out
        );
    }
    if let Some(error) = &summary.postprocessing_error {
        bail!("worker result recorded; postprocessing failed: {error}");
    }
    Ok(())
}

fn require_successful_resume(result: &serde_json::Value) -> Result<()> {
    let summary = if let Some(worker) = result.get("worker") {
        let state: crate::control::WorkerStateV1 = serde_json::from_value(worker["state"].clone())
            .context("resume returned no valid durable worker state")?;
        anyhow::ensure!(
            state != crate::control::WorkerStateV1::InDoubt,
            "worker remains in doubt; reservation and runtime evidence were retained"
        );
        // Reconciliation can succeed without a recoverable command result.
        // Such output stays unknown; an existing result still controls CLI failure.
        match result.get("result").filter(|value| !value.is_null()) {
            Some(value) => value,
            None => return Ok(()),
        }
    } else {
        result // A recovered first start returns the same summary as `run`.
    };
    let summary: runner::RunSummary = serde_json::from_value(summary.clone())
        .context("resume returned an invalid command result")?;
    require_successful_run(&summary)
}

pub async fn execute(command: OperatorCommand) -> Result<()> {
    use crate::runner::AuditClient;
    // Build, Up and Down hold a prepared source across engine commands, and
    // Release holds one while it reads and hashes: run them inside one
    // operator-interruption scope. Every other command, including
    // VerifyAudit, keeps its existing flow.
    if source_snapshot::needs_interrupt_scope(&command) {
        return source_snapshot::interrupt_scope(Box::pin(execute(command))).await;
    }
    match command {
        OperatorCommand::Init { config_dir } => {
            let config = initialize_config_directory(&config_dir)?;
            println!(
                "Configuration ready at {}. Authentication keys were preserved or created; no key was printed.",
                config.path().display()
            );
        }
        OperatorCommand::Providers { command } => match command {
            ProviderCommand::Init {
                config_dir,
                env_file,
            } => {
                initialize_provider_references(&config_dir, &env_file)?;
                println!(
                    "Private provider references created. Credential contents and worker capabilities have not been checked; use providers status to inspect the configured choices."
                );
            }
            ProviderCommand::Status {
                config_dir,
                providers_file,
                json: _,
            } => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&provider_status(
                        &config_dir,
                        providers_file.as_deref()
                    )?)?
                );
            }
        },
        OperatorCommand::Doctor {
            runtime,
            runtime_program,
            runtime_endpoint,
            allow_rootful_runtime,
        } => {
            let runtime = selected_runtime(
                runtime,
                runtime_program.as_deref(),
                runtime_endpoint.as_deref(),
                None,
            )
            .await?;
            let info = runtime.doctor(allow_rootful_runtime).await?;
            let version = runtime
                .cli(&["version".into()], Duration::from_secs(15))
                .await?;
            println!(
                "{}\n{}",
                serde_json::to_string_pretty(&info)?,
                String::from_utf8_lossy(&version.bytes)
            );
        }
        OperatorCommand::Build {
            runtime,
            runtime_program,
            runtime_endpoint,
            source,
            allow_rootful_runtime,
            codex_version,
            claude_code_version,
            build_revision,
        } => {
            if let Some(version) = &codex_version {
                validate_provider_version(version)?;
            }
            if let Some(version) = &claude_code_version {
                validate_provider_version(version)?;
            }
            let source = source.canonicalize()?;
            preflight_build_inputs(&source)?;
            let prepared = source_snapshot::prepare_build(&source).await?;
            // Hold the common plan for the whole build. Read/hash off the
            // signal task and resolve both commands before probing the engine.
            let (_plan, commands) = prepared
                .prepare_build_plan(
                    build_revision.as_deref(),
                    codex_version.as_deref(),
                    claude_code_version.as_deref(),
                )
                .await?;
            let runtime = selected_runtime(
                runtime,
                runtime_program.as_deref(),
                runtime_endpoint.as_deref(),
                Some(&source),
            )
            .await?;
            runtime.doctor(allow_rootful_runtime).await?;
            for (tag, command) in ["agentsandbox-control:local", "agentsandbox-worker:local"]
                .into_iter()
                .zip(commands)
            {
                eprintln!("Building {tag} with {}…", runtime.kind);
                let output = runtime
                    .consume_prepared(command, Duration::from_secs(30 * 60), &[], None)
                    .await?;
                println!("{}", String::from_utf8_lossy(&output.bytes));
                if output.truncated {
                    eprintln!(
                        "Build log was truncated to 256 KiB; runtime reported successful completion."
                    );
                }
            }
        }
        OperatorCommand::Up {
            runtime,
            runtime_program,
            runtime_endpoint,
            source,
            config_dir,
            allow_rootful_runtime,
        } => {
            let config = initialize_config_directory(&config_dir)?;
            let source = source.canonicalize()?;
            let runtime = selected_runtime(
                runtime,
                runtime_program.as_deref(),
                runtime_endpoint.as_deref(),
                Some(&source),
            )
            .await?;
            let info = runtime.doctor(allow_rootful_runtime).await?;
            if !info.rootless {
                eprintln!(
                    "WARNING: explicitly using a runtime that does not report rootless mode."
                );
            }
            // Atomic projection refresh changes file inodes. Recreate services
            // so both the bind mounts and startup-loaded keys are refreshed.
            compose(
                &runtime,
                &source,
                &config,
                &["up", "--detach", "--force-recreate"],
            )
            .await?;
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .no_proxy()
                .build()?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
            loop {
                let gateway = http
                    .get("http://127.0.0.1:8787/healthz")
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success());
                let dashboard = http
                    .get("http://127.0.0.1:4000/healthz")
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success());
                if gateway && dashboard {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    bail!(
                        "stack started but readiness did not pass within 45 seconds; inspect {} compose logs",
                        runtime.kind
                    );
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            println!(
                "Gateway: http://127.0.0.1:8787\nDashboard: http://127.0.0.1:4000\nUse the viewer key from {} to sign into the dashboard.",
                config.path().join("secrets/viewer").display()
            );
        }
        OperatorCommand::Down {
            runtime,
            runtime_program,
            runtime_endpoint,
            source,
            config_dir,
        } => {
            let config = crate::control_fs::Directory::open_existing(
                &crate::control_fs::absolute_path(&config_dir)?,
            )?;
            let source = source.canonicalize()?;
            let runtime = selected_runtime(
                runtime,
                runtime_program.as_deref(),
                runtime_endpoint.as_deref(),
                Some(&source),
            )
            .await?;
            compose(&runtime, &source, &config, &["down"]).await?;
            println!("Control-plane services stopped. Audit and session volumes retained.");
        }
        OperatorCommand::Run(options) => {
            let summary = runner::run(*options).await?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            require_successful_run(&summary)?;
        }
        OperatorCommand::WorkerStatus {
            session,
            config_dir,
            gateway,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &runner::worker_status(session, &config_dir, &gateway).await?
                )?
            );
        }
        OperatorCommand::WorkerResume {
            interactive,
            runtime_program,
            adopt_runtime_program,
            session,
            config_dir,
            gateway,
            wait_seconds,
        } => {
            let result = runner::resume_worker_with_mode(
                session,
                &config_dir,
                &gateway,
                wait_seconds,
                runtime_program.as_deref(),
                adopt_runtime_program,
                interactive,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            require_successful_resume(&result)?;
        }
        OperatorCommand::WorkerStop {
            runtime_program,
            adopt_runtime_program,
            session,
            config_dir,
            gateway,
        } => {
            let result = runner::stop_worker_with_program(
                session,
                &config_dir,
                &gateway,
                runtime_program.as_deref(),
                adopt_runtime_program,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            if result["worker"]["state"] == "in_doubt" {
                bail!("stop is not proven terminal; reservation remains held");
            }
        }
        OperatorCommand::ExportWorkspace {
            runtime_program,
            adopt_runtime_program,
            session,
            output,
            config_dir,
            gateway,
        } => {
            let count = runner::export_workspace_with_program(
                session,
                output.clone(),
                &config_dir,
                &gateway,
                runtime_program.as_deref(),
                adopt_runtime_program,
            )
            .await?;
            println!("Exported {count} verified files into {}", output.display());
        }
        OperatorCommand::VerifyAudit {
            config_dir,
            viewer_token_file,
            gateway,
            checkpoint_dir,
            anchor,
            data,
        } => {
            let viewer_token_file =
                viewer_token_file.unwrap_or_else(|| config_dir.join("secrets/viewer"));
            let token = runner::read_token(&viewer_token_file)?;
            let answer = AuditClient::new(&gateway, &token)?
                .get_json("api/integrity")
                .await?;
            println!("{}", serde_json::to_string_pretty(&answer)?);
            if answer.get("valid").and_then(serde_json::Value::as_bool) != Some(true) {
                bail!("audit chain verification failed");
            }
            if let (Some(checkpoint_dir), Some(anchor)) = (checkpoint_dir, anchor) {
                use crate::audit::signed_checkpoint::{RecordSource, VerifyRequest, cli};
                let signed = cli::verification(&VerifyRequest {
                    anchor: &anchor,
                    source: RecordSource::Directory(&checkpoint_dir),
                    data: data.as_deref(),
                    require_reference: false,
                    report_out: None,
                });
                println!("{}", signed.stdout);
                if signed.exit_code != 0 {
                    bail!("signed audit checkpoint verification failed");
                }
            }
        }
        OperatorCommand::Release { command } => source_snapshot::release(command).await?,
    }
    Ok(())
}

struct OperatorRuntime {
    kind: RuntimeKind,
    approved: crate::runtime::Runtime,
    docker: Option<crate::runtime::ManagedRuntime>,
}
impl OperatorRuntime {
    async fn doctor(&self, allow_rootful: bool) -> Result<crate::runtime::RuntimeInfo> {
        match &self.docker {
            Some(runtime) => runtime.doctor(allow_rootful).await,
            None => self.approved.doctor(allow_rootful).await,
        }
    }
    async fn cli(&self, args: &[String], duration: Duration) -> Result<crate::runtime::Capture> {
        self.cli_env(args, duration, &[], None).await
    }
    async fn cli_env(
        &self,
        args: &[String],
        duration: Duration,
        environment: &[(&str, &str)],
        directory: Option<&crate::control_fs::Directory>,
    ) -> Result<crate::runtime::Capture> {
        let mut targeted = Vec::new();
        if let Some(runtime) = &self.docker {
            // This is the same read-only identity check used by managed recovery.
            // The check and CLI effect are separate engine connections; this does
            // not provide atomic fencing against an administrator replacing it.
            crate::runtime::ManagedRuntime::reopen_with_runtime(
                runtime.binding().clone(),
                self.approved.clone(),
            )
            .await?;
            targeted.extend(["--host".into(), runtime.binding().endpoint.clone()]);
        }
        targeted.extend_from_slice(args);
        if let Some(directory) = directory {
            directory.revalidate()?;
        }
        self.approved
            .cli_env(&targeted, duration, environment)
            .await
    }
}
async fn selected_runtime(
    kind: RuntimeKind,
    program: Option<&Path>,
    endpoint: Option<&str>,
    source: Option<&Path>,
) -> Result<OperatorRuntime> {
    let runtime = match program {
        Some(program) => crate::runtime::Runtime::with_program(kind, program)?,
        None => crate::runtime::Runtime::new(kind)?,
    };
    if let Some(source) = source {
        runner::exclude_project_runtime(&runtime, source)?;
    }
    let docker = if kind == RuntimeKind::Docker {
        let endpoint = runner::docker_endpoint(&runtime, endpoint).await?;
        Some(
            crate::runtime::ManagedRuntime::connect_docker_with_runtime(
                uuid::Uuid::new_v4(),
                &endpoint,
                runtime.clone(),
            )
            .await?,
        )
    } else {
        anyhow::ensure!(
            endpoint.is_none(),
            "operator --runtime-endpoint requires Docker; managed Podman sessions accept local or Unix endpoints through run"
        );
        None
    };
    Ok(OperatorRuntime {
        kind,
        approved: runtime,
        docker,
    })
}

#[cfg(unix)]
pub fn initialize_config(path: &Path) -> Result<()> {
    initialize_config_directory(path).map(|_| ())
}
#[cfg(unix)]
fn initialize_config_directory(path: &Path) -> Result<crate::control_fs::Directory> {
    let directory =
        crate::control_fs::Directory::open_or_create(&crate::control_fs::absolute_path(path)?)?;
    let sources_guard = directory.child("secrets")?;
    let projected_guard = directory.child("mounted-secrets")?;
    for child in ["sessions", "pending"] {
        directory.child(child)?;
    }
    directory.revalidate()?;
    let sources = secret_projections::open_root(&sources_guard)?;
    let projected = secret_projections::open_root(&projected_guard)?;
    let runner = secret_projections::source(&sources, "runner", true)?
        .context("runner source was not created")?;
    let viewer = secret_projections::source(&sources, "viewer", true)?
        .context("viewer source was not created")?;
    let jev = secret_projections::source(&sources, "jev", false)?;
    if runner == viewer {
        bail!("runner and viewer keys must be distinct; existing values were not changed");
    }
    if let Some(key) = &jev {
        crate::jev::validate_credential_separation(key, [runner.as_str(), viewer.as_str()])?;
    }
    let mut source_files = Vec::new();
    for (role, value) in [
        ("runner", Some(runner.as_str())),
        ("viewer", Some(viewer.as_str())),
        ("jev", jev.as_deref()),
    ] {
        if let Some(value) = value {
            let source = sources_guard.read_private_file(role, 514)?;
            anyhow::ensure!(
                std::str::from_utf8(source.bytes())?.trim() == value,
                "control secret source changed during initialization; existing values were preserved"
            );
            source_files.push(source);
        }
    }
    for role in ["runner", "viewer", "jev"] {
        secret_projections::validate_existing(&projected, role)?;
    }
    secret_projections::publish(&projected, "runner", &runner)?;
    secret_projections::publish(&projected, "viewer", &viewer)?;
    if let Some(jev) = jev {
        secret_projections::publish(&projected, "jev", &jev)?;
    } else {
        secret_projections::remove_optional(&projected, "jev")?;
    }
    sources_guard.revalidate()?;
    projected_guard.revalidate()?;
    directory.revalidate()?;
    for source in source_files {
        source.revalidate()?;
    }
    Ok(directory)
}

#[cfg(not(unix))]
pub fn initialize_config(_path: &Path) -> Result<()> {
    bail!("secure secret initialization currently requires Linux, macOS, or WSL")
}

#[cfg(unix)]
mod secret_projections {
    use super::*;
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, PermissionsExt},
        },
    };
    fn check_directory(file: &fs::File) -> Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o7777 != 0o700
        {
            bail!("secret directory must be owned by the current user and have mode 0700");
        }
        Ok(())
    }
    pub(super) fn open_root(directory: &crate::control_fs::Directory) -> Result<fs::File> {
        let file = directory.try_clone_file()?;
        check_directory(&file)?;
        Ok(file)
    }
    fn open_existing(directory: &fs::File, name: &str) -> Result<Option<fs::File>> {
        let name = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(error).context("secret entry is inaccessible or is a symlink");
        }
        let file = unsafe { fs::File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.len() > 514
        {
            bail!(
                "secret entry must be a small regular file owned by the current user, without hard links"
            );
        }
        Ok(Some(file))
    }
    pub(super) fn source(directory: &fs::File, role: &str, create: bool) -> Result<Option<String>> {
        let existing = open_existing(directory, role)?;
        let mut file = if let Some(file) = existing {
            file
        } else {
            if !create {
                return Ok(None);
            }
            let mut entropy = [0_u8; 32];
            fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
            let name = CString::new(role)?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("secret source appeared concurrently; existing value was preserved");
            }
            let mut file = unsafe { fs::File::from_raw_fd(fd) };
            writeln!(file, "{}", hex::encode(entropy))?;
            file.sync_all()?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            file
        };
        let mode = file.metadata()?.permissions().mode() & 0o7777;
        if ![0o400, 0o444, 0o600].contains(&mode) {
            bail!("secret source permissions must be 0600; no source value was overwritten");
        }
        // Migrate the previous release's 0444 source mode without rotating the
        // key. All future original sources remain owner-readable/writable only.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let mut content = String::new();
        (&mut file).take(515).read_to_string(&mut content)?;
        let token = content.trim();
        if token.len() < 32
            || token.len() > 512
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            bail!("secret source must contain 32 to 512 non-whitespace ASCII characters");
        }
        Ok(Some(token.to_owned()))
    }
    pub(super) fn validate_existing(directory: &fs::File, role: &str) -> Result<()> {
        if let Some(file) = open_existing(directory, role)? {
            let mode = file.metadata()?.permissions().mode() & 0o7777;
            if ![0o444, 0o600].contains(&mode) {
                bail!("mounted secret projection has unexpected permissions");
            }
        }
        Ok(())
    }
    pub(super) fn publish(directory: &fs::File, role: &str, token: &str) -> Result<()> {
        let temporary = CString::new(format!(".{role}-{}.tmp", uuid::Uuid::new_v4()))?;
        let target = CString::new(role)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("cannot create private secret projection");
        }
        let result = (|| -> Result<()> {
            let mut file = unsafe { fs::File::from_raw_fd(fd) };
            writeln!(file, "{token}")?;
            file.set_permissions(fs::Permissions::from_mode(0o444))?;
            file.sync_all()?;
            if unsafe {
                libc::renameat(
                    directory.as_raw_fd(),
                    temporary.as_ptr(),
                    directory.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error())
                    .context("cannot atomically publish secret projection");
            }
            directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result
    }
    pub(super) fn remove_optional(directory: &fs::File, role: &str) -> Result<()> {
        let target = CString::new(role)?;
        if unsafe { libc::unlinkat(directory.as_raw_fd(), target.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ENOENT) {
                return Err(error.into());
            }
        }
        Ok(())
    }
}
fn preflight_build_inputs(source: &Path) -> Result<()> {
    const DIRECTORIES: &[&str] = &[
        "images",
        "src",
        "src/bin",
        "front",
        "front/src",
        "front/public",
        "policies",
    ];
    const FILES: &[&str] = &[
        "images/control.Dockerfile",
        "images/worker.Dockerfile",
        ".dockerignore",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "LICENSE",
        "src/main.rs",
        "src/lib.rs",
        "src/bin/ai-guard-provider.rs",
        "front/package.json",
        "front/package-lock.json",
        "front/tsconfig.json",
        "front/vite.config.ts",
        "front/index.html",
        "front/src/main.ts",
        "front/src/style.css",
        "front/public/favicon.svg",
        "policies/default.json",
    ];
    // Structural precheck, run on the selected source and again on its prepared
    // copy. source_snapshot owns the bounded reads and the content binding.
    for (paths, directory) in [(DIRECTORIES, true), (FILES, false)] {
        for relative in paths {
            let metadata = fs::symlink_metadata(source.join(relative))
                .map_err(|_| anyhow::anyhow!("required build input is unavailable: {relative}"))?;
            if metadata.file_type().is_symlink() {
                bail!("required build input must not be a symbolic link: {relative}");
            }
            let valid = if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            };
            if !valid {
                let kind = if directory {
                    "directory"
                } else {
                    "regular file"
                };
                bail!("required build input must be a {kind}: {relative}");
            }
        }
    }
    Ok(())
}

pub fn validate_provider_version(version: &str) -> Result<()> {
    let exact = regex::Regex::new(
        r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z.-]+)?$",
    )?;
    if version.len() > 100 || !exact.is_match(version) {
        bail!(
            "provider versions must be exact semantic versions; latest, ranges, and shell syntax are forbidden"
        );
    }
    Ok(())
}
async fn compose(
    runtime: &OperatorRuntime,
    source: &Path,
    config: &crate::control_fs::Directory,
    action: &[&str],
) -> Result<()> {
    let source = source.canonicalize()?;
    config.revalidate()?;
    let file = source
        .join("deploy")
        .join(runtime.kind.program())
        .join("compose.yaml");
    if !file.is_file() {
        bail!("compose file is missing: {}", file.display());
    }
    let mut args = vec![
        "compose".into(),
        "--project-name".into(),
        "agentsandbox".into(),
    ];
    args.extend(action.iter().map(|s| s.to_string()));
    let prepared = source_snapshot::prepare_compose(&source, runtime.kind).await?;
    let command = prepared.compose_command(&args)?;
    let config_path = config
        .path()
        .to_str()
        .context("configuration path must be UTF-8")?;
    let output = runtime
        .consume_prepared(
            command,
            Duration::from_secs(180),
            &[("AGENTSANDBOX_CONFIG_DIR", config_path)],
            Some(config),
        )
        .await?;
    println!("{}", String::from_utf8_lossy(&output.bytes));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn provider_status_exposes_launch_only_mediation_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let config_dir = root.join(".agentsandbox");
        let env_file = root.join("private-provider.env");
        initialize_provider_references(&config_dir, &env_file).unwrap();

        let status = provider_status(&config_dir, None).unwrap();
        assert_eq!(status["version"], 1);
        assert_eq!(status["status"], "configured");
        assert_eq!(
            status["mediation"],
            serde_json::json!({"scope": "launch_only", "tool_decision_channel": false})
        );
        assert!(!status.to_string().contains("private-provider.env"));
    }
}
