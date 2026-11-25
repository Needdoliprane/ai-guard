//! Real Docker Compose rehearsal on a fresh project and private configuration.
#![cfg(unix)]

use agentsandbox::runtime::{Runtime, RuntimeKind};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpListener,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const COMPOSE: &str = include_str!("../deploy/docker/compose.yaml");
// Port adaptation (G01 reconciliation, 2026-11-25): the integration Compose
// parameterizes the control image; match that declaration instead of the
// literal image line of the 2026-11-18 snapshot. Still exactly three services.
const DEFAULT_IMAGE: &str = "    image: ${AI_GUARD_CONTROL_IMAGE:-agentsandbox-control:local}\n";

struct Stack {
    runtime: Runtime,
    endpoint: String,
    name: String,
    evidence: PathBuf,
    gateway_port: u16,
    dashboard_port: u16,
    allow_rootful: bool,
}

impl Stack {
    async fn read_engine(&self, args: &[&str]) -> Result<String> {
        let arguments = ["--host", &self.endpoint]
            .into_iter()
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let output = self
            .runtime
            .cli(&arguments, Duration::from_secs(15))
            .await?;
        ensure!(
            !output.truncated,
            "Docker inventory exceeded its response bound"
        );
        Ok(String::from_utf8(output.bytes)?.trim().to_owned())
    }

    async fn project_resources(&self, args: &[&str]) -> Result<Vec<String>> {
        let label = format!("label=com.docker.compose.project={}", self.name);
        let argv = [args, &["--filter", &label, "--quiet"]].concat();
        let inventory = self.read_engine(&argv).await?;
        let mut rows: Vec<_> = inventory.lines().map(str::to_owned).collect();
        rows.sort_unstable();
        Ok(rows)
    }

    fn cli(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(operation)
            .arg("--config-dir")
            .arg(self.evidence.join("config"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    async fn compose_action(&self, action: &str) -> Result<String> {
        let mut command = self.cli(action);
        command
            .args(["--runtime", "docker", "--runtime-endpoint", &self.endpoint])
            .args(["--stack-name", &self.name, "--source"])
            .arg(self.evidence.join("source"))
            .arg("--runtime-program")
            .arg(&self.runtime.invocation().approved_entry);
        if action == "up" {
            command.args(["--gateway-port", &self.gateway_port.to_string()]);
            command.args(["--dashboard-port", &self.dashboard_port.to_string()]);
            if self.allow_rootful {
                command.arg("--allow-rootful-runtime");
            }
        }
        cli_output(command, action).await
    }

    async fn audit_head(&self) -> Result<Value> {
        let mut command = self.cli("verify-audit");
        let origin = format!("http://127.0.0.1:{}", self.gateway_port);
        command.args(["--gateway", &origin]);
        let output = cli_output(command, "verify-audit").await?;
        let audit: Value =
            serde_json::from_str(&output).context("invalid audit verification JSON")?;
        ensure!(
            audit["valid"] == true,
            "audit verification did not report valid=true"
        );
        let count = audit["event_count"]
            .as_u64()
            .context("missing audit count")?;
        let head = audit["head_hash"].as_str().context("missing audit head")?;
        ensure!(
            count > 0 && hex_digest(head),
            "no durable audit head was observed"
        );
        Ok(json!({"valid": true, "event_count": count, "head_hash": head}))
    }

    async fn exercise(&self, image: &str, report: &mut Value) -> Result<()> {
        report["stage"] = json!("preflight");
        for args in [
            ["container", "ls", "--all"].as_slice(),
            &["volume", "ls"],
            &["network", "ls"],
        ] {
            ensure!(
                self.project_resources(args).await?.is_empty(),
                "test project is already in use"
            );
        }
        let image_id = self
            .read_engine(&["image", "inspect", "--format", "{{.Id}}", image])
            .await?;
        ensure!(
            image_id.strip_prefix("sha256:").is_some_and(hex_digest),
            "invalid control image ID"
        );
        report["control_image_id"] = json!(image_id);
        ensure!(
            COMPOSE.matches(DEFAULT_IMAGE).count() == 3,
            "unexpected control image declarations in the Compose source"
        );
        let compose_dir = self.evidence.join("source/deploy/docker");
        fs::create_dir_all(&compose_dir)?;
        let image_declaration = format!("    image: \"{image}\"\n    pull_policy: never\n");
        fs::write(
            compose_dir.join("compose.yaml"),
            COMPOSE.replace(DEFAULT_IMAGE, &image_declaration),
        )?;

        let mut retained_volume = None;
        let mut original_head = None;
        let container_query = ["container", "ls", "--all"];
        let volume_query = ["volume", "ls"];
        for phase in ["initial", "restarted"] {
            report["stage"] = json!(format!("{phase} up"));
            eprintln!("Stack {} stage {phase} up", self.name);
            let output = self.compose_action("up").await?;
            ensure!(
                output
                    .lines()
                    .any(|line| line == format!("Gateway: http://127.0.0.1:{}", self.gateway_port)),
                "up did not announce the expected gateway"
            );
            ensure!(
                output
                    .lines()
                    .any(|line| line
                        == format!("Dashboard: http://127.0.0.1:{}", self.dashboard_port)),
                "up did not announce the expected dashboard"
            );

            report["stage"] = json!(format!("{phase} audit"));
            let containers = self.project_resources(&container_query).await?;
            ensure!(
                containers.len() == 3,
                "expected three control services in the test project"
            );
            for id in containers {
                let actual_id = self
                    .read_engine(&["container", "inspect", "--format", "{{.Image}}", &id])
                    .await?;
                ensure!(
                    actual_id == image_id,
                    "a control service used a different image"
                );
            }
            let volumes = self.project_resources(&volume_query).await?;
            ensure!(
                volumes.len() == 1,
                "expected one private project audit volume"
            );
            let head = self.audit_head().await?;
            report[phase] = json!({"audit": head, "volumes": volumes});
            if let (Some(before), Some(original)) = (&original_head, &retained_volume) {
                ensure!(&head == before, "the audit head changed on restart");
                ensure!(
                    &volumes == original,
                    "the project did not reuse its audit volume"
                );
            } else {
                original_head = Some(head);
                retained_volume = Some(volumes.clone());
            }

            report["stage"] = json!(format!("{phase} down"));
            eprintln!("Stack {} stage {phase} down", self.name);
            self.compose_action("down").await?;
            ensure!(
                self.project_resources(&container_query).await?.is_empty(),
                "down left control containers in the project"
            );
            ensure!(
                volumes == self.project_resources(&volume_query).await?,
                "down did not retain the audit volume"
            );
        }
        report["stage"] = json!("complete");
        Ok(())
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn cli_output(mut command: Command, stage: &str) -> Result<String> {
    let output = tokio::time::timeout(Duration::from_secs(300), command.output())
        .await
        .with_context(|| format!("CLI {stage} timed out"))??;
    ensure!(
        output.status.success(),
        "CLI {stage} failed ({})",
        output.status
    );
    Ok(String::from_utf8(output.stdout).context("CLI output is not UTF-8")?)
}

#[tokio::test]
#[ignore = "requires a real Docker engine, local control image and explicitly selected evidence directory"]
async fn cli_restarts_a_dedicated_control_stack_without_losing_audit() -> Result<()> {
    let endpoint = std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT")
        .context("select a local Docker Unix endpoint")?;
    ensure!(
        endpoint.starts_with("unix:///") && !endpoint.chars().any(char::is_control),
        "an explicit local Docker Unix endpoint is required"
    );
    let image = std::env::var("AGENTSANDBOX_TEST_CONTROL_IMAGE")
        .context("select a locally built control image")?;
    ensure!(
        !image.is_empty()
            && !image.starts_with('-')
            && image
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/@".contains(&byte)),
        "invalid control image reference"
    );
    let evidence = PathBuf::from(
        std::env::var_os("AGENTSANDBOX_TEST_EVIDENCE_DIR")
            .context("select a fresh private evidence directory")?,
    );
    ensure!(
        evidence.is_absolute() && evidence.file_name().is_some(),
        "the new evidence directory must have an absolute path"
    );
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&evidence)
        .context("create a fresh private evidence directory")?;
    let evidence = evidence.canonicalize()?;
    let runtime = match std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM") {
        Some(program) => Runtime::with_program(RuntimeKind::Docker, Path::new(&program))?,
        None => Runtime::new(RuntimeKind::Docker)?,
    };
    let gateway = TcpListener::bind("127.0.0.1:0")?;
    let dashboard = TcpListener::bind("127.0.0.1:0")?;
    let stack = Stack {
        runtime,
        endpoint,
        name: format!("aigqa-{}", uuid::Uuid::new_v4().simple()),
        evidence,
        gateway_port: gateway.local_addr()?.port(),
        dashboard_port: dashboard.local_addr()?.port(),
        allow_rootful: std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1"),
    };
    // Compose must own both binds. Concurrent use of either port fails the test.
    drop((gateway, dashboard));
    eprintln!("Stack {} evidence {}", stack.name, stack.evidence.display());
    let mut report = json!({
        "stack": stack.name,
        "gateway_port": stack.gateway_port,
        "dashboard_port": stack.dashboard_port,
        "control_image": image,
        "allow_rootful_runtime": stack.allow_rootful,
    });
    let outcome = stack.exercise(&image, &mut report).await;
    report["status"] = json!(if outcome.is_ok() { "PASS" } else { "FAIL" });
    fs::write(
        stack.evidence.join("control-stack-result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    outcome.with_context(|| format!("stack {} stage {}", stack.name, report["stage"]))
}
