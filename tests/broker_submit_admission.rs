//! Real broker CLI and gateway admission with a synthetic managed Codex profile.
//! The fixture refuses the first worker transition after admission. The local
//! runtime witness never invokes a container engine or a provider.
#![cfg(any(target_os = "linux", target_os = "macos"))]

#[allow(dead_code, unused_imports)]
#[path = "support/broker_relay_handoff.rs"]
mod harness;

use agentsandbox::{
    audit::EventQuery,
    broker::service::protocol::{BrokerErrorCodeV1, BrokerReplyV1, JobStateV1, JobStatusV1},
    control::{SupervisedAdmissionV1, WorkerStateV1, content_hash},
    model::Outcome,
    provider::{AuthMode, ClientKind, ClientTransport},
    workspace::{Limits, extract_workspace},
};
use anyhow::{Context, Result, bail, ensure};
use harness::{BIN, Fixture};
use std::{
    fs::{self, File},
    io,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;

const MODEL: &str = "gpt-6-luna";
const PROJECT_FILE: &str = "original.txt";
const PROJECT_CONTENT: &[u8] = b"synthetic admission source remains unchanged\n";
const CLI_STREAM_LIMIT: u64 = 64 * 1024;

fn broker_cli(fixture: &Fixture, action: &str, operation: Uuid) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["broker", action, "--root"])
        .arg(&fixture.broker_root)
        .args(["--profile", "a", "--token-file"])
        .arg(fixture.runner_file())
        .arg("--operation-id")
        .arg(operation.to_string())
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .current_dir(&fixture.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn capture_cli(stream: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .take(CLI_STREAM_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

fn cli_failure_category(stderr: &[u8]) -> &'static str {
    // CLI diagnostics may contain fixture values. Only emit fixed categories.
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.contains("broker reply is unavailable or invalid")
        || stderr.contains("incompatible broker reply version")
    {
        "ipc_reply_unavailable_or_invalid"
    } else if stderr.contains("broker request outcome is unknown") {
        "ipc_request_outcome_unknown"
    } else if stderr.contains("broker is unavailable; request was not sent")
        || stderr.contains("broker is not running")
    {
        "ipc_unavailable_before_request"
    } else if stderr.contains("Usage:") && stderr.contains("error:") {
        "cli_arguments"
    } else {
        "unclassified_cli_failure"
    }
}

async fn read_reply(mut command: Command, action: &str) -> Result<BrokerReplyV1> {
    let mut child = command.spawn().map_err(|error| {
        anyhow::anyhow!(
            "broker {action} category=spawn_failed kind={:?}",
            error.kind()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .with_context(|| format!("broker {action} category=missing_cli_stdout_pipe"))?;
    let stderr = child
        .stderr
        .take()
        .with_context(|| format!("broker {action} category=missing_cli_stderr_pipe"))?;
    let (status, stdout, stderr) = timeout(Duration::from_secs(20), async {
        tokio::try_join!(child.wait(), capture_cli(stdout), capture_cli(stderr))
    })
    .await
    .map_err(|_| anyhow::anyhow!("broker {action} category=cli_harness_timeout"))?
    .map_err(|error| {
        anyhow::anyhow!(
            "broker {action} category=cli_capture_failed kind={:?}",
            error.kind()
        )
    })?;
    let exit = match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit_code={code}"),
        (_, Some(signal)) => format!("signal={signal}"),
        _ => "exit_status=unknown".into(),
    };
    ensure!(
        stdout.len() as u64 <= CLI_STREAM_LIMIT && stderr.len() as u64 <= CLI_STREAM_LIMIT,
        "broker {action} {exit} category=cli_output_limit"
    );
    let reply = serde_json::from_slice::<BrokerReplyV1>(&stdout).ok();
    match (status.success(), reply) {
        (_, Some(BrokerReplyV1::Error { code })) => {
            bail!("broker {action} {exit} category=broker_error code={code:?}");
        }
        (_, Some(BrokerReplyV1::Rejected { phase, code, .. })) => {
            bail!("broker {action} {exit} category=broker_rejected phase={phase:?} code={code:?}");
        }
        (true, Some(reply)) => Ok(reply),
        (true, None) => bail!("broker {action} {exit} category=missing_or_invalid_cli_reply"),
        (false, Some(_)) => bail!("broker {action} {exit} category=typed_reply_with_nonzero_exit"),
        (false, None) => bail!(
            "broker {action} {exit} category={}",
            cli_failure_category(&stderr)
        ),
    }
}

async fn wait_for_prepare_refusal(fixture: &Fixture, operation: Uuid) -> Result<JobStatusV1> {
    timeout(Duration::from_secs(30), async {
        loop {
            let BrokerReplyV1::Status { status } =
                read_reply(broker_cli(fixture, "status", operation), "status").await?
            else {
                bail!("broker status exit_code=0 category=unexpected_status_reply");
            };
            ensure!(
                status.operation_id == operation,
                "uncorrelated broker status"
            );
            if !status.executor_task_running
                && status.executor_error == Some(BrokerErrorCodeV1::NeedsReconciliation)
                && status.failure_count > 0
            {
                return Ok(status);
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("broker status category=prepare_refusal_not_confirmed")?
}

fn inspect_archive(fixture: &Fixture, project: &Path, operation: Uuid) -> Result<String> {
    let archive = fixture
        .root
        .join("configs/a/sessions")
        .join(format!("{operation}.tar"));
    let metadata = fs::symlink_metadata(&archive).context("immutable workspace archive missing")?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.permissions().mode() & 0o077 == 0,
        "workspace archive is not a private regular file"
    );
    let destination = fixture.root.join("verified-archive");
    let manifest = extract_workspace(
        File::open(archive)?,
        &destination,
        project,
        Limits::default(),
    )?;
    ensure!(
        manifest.len() == 1
            && manifest.contains_key(PROJECT_FILE)
            && fs::read(destination.join(PROJECT_FILE))?.as_slice() == PROJECT_CONTENT,
        "the private archive did not contain exactly the synthetic original"
    );
    content_hash("ai-guard:workspace-manifest:v1", &manifest)
}

async fn verify_admission(fixture: &Fixture) -> Result<()> {
    let project = fixture.root.join("sources");
    let original = project.join(PROJECT_FILE);
    fs::write(&original, PROJECT_CONTENT)?;
    let operation = Uuid::new_v4();
    let operation_id = operation.to_string();
    let mut submit = broker_cli(fixture, "submit", operation);
    submit.arg("--project").arg(&project).args([
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--client-profile",
        "codex-api",
        "--model",
        MODEL,
        "--prompt",
        "Read original.txt in the synthetic workspace.",
    ]);
    let BrokerReplyV1::Accepted {
        operation_id: accepted,
        intent_hash,
        revision,
        replayed: false,
        provider_resolution: Some(resolution),
        ..
    } = read_reply(submit, "submit").await?
    else {
        bail!("broker submit exit_code=0 category=missing_fresh_resolved_acceptance");
    };
    ensure!(
        accepted == operation
            && resolution.client == ClientKind::Codex
            && resolution.auth_mode == AuthMode::Api
            && resolution.transport == ClientTransport::Batch
            && resolution.profile_id == "codex-api"
            && resolution.model.as_deref() == Some(MODEL),
        "the accepted submission did not bind the chosen Codex API batch profile"
    );
    eprintln!("broker submit confirmed the fresh managed job");

    // The real executor can request Prepare only after it receives and
    // persists the admission response. The fixture refuses this transition.
    let prepare = fixture.wait_before_import().await?;
    eprintln!("gateway fixture observed the managed Prepare request");
    let status = wait_for_prepare_refusal(fixture, operation).await?;
    ensure!(
        status.operation_id == operation
            && status.state == JobStateV1::Accepted
            && status.intent_hash.as_deref() == Some(intent_hash.as_str())
            && status.head_revision.is_some_and(|head| head > revision)
            && status
                .last_decision
                .is_some_and(|decision| decision.outcome == Outcome::Allow)
            && status.worker_state == Some(WorkerStateV1::Reserved)
            && status.quota_held == Some(true),
        "the broker journal did not project the durable Allow and reserved worker"
    );

    let admission = fixture
        .a
        .worker_operation(&operation_id)?
        .context("the admission receipt is absent from SQLite")?;
    let worker_id = admission
        .worker_id
        .as_deref()
        .context("the real admission was refused")?;
    let workers = fixture.a.workers()?;
    let [worker] = workers.as_slice() else {
        bail!("the selected tenant must have exactly one worker");
    };
    ensure!(
        admission.operation_id == operation_id
            && worker.worker_id == worker_id
            && worker.admission_operation_id == operation_id
            && worker.launch.provider_resolution.as_ref() == Some(&resolution)
            && Uuid::parse_str(&worker.launch.session_id).is_ok_and(|session| !session.is_nil())
            && worker.request_id == worker.receipt.request_id
            && prepare.worker_id == worker_id
            && prepare.attempt_id == worker.attempt.attempt_id
            && prepare.expected_revision == worker.revision
            && fixture.a.worker_operation(&prepare.operation_id)?.is_none()
            && admission.revision == Some(worker.revision)
            && worker.state == WorkerStateV1::Reserved
            && worker.quota_held
            && worker.attempt.create_operation_id.is_none()
            && worker.attempt.start_operation_id.is_none()
            && worker.attempt.container_id.is_none()
            && worker.last_observation.is_none()
            && fixture.b.workers()?.is_empty(),
        "SQLite, the reserved worker and the broker's next request did not identify one admission"
    );
    ensure!(
        inspect_archive(fixture, &project, operation)? == worker.launch.workspace_manifest_digest,
        "the durable admission did not bind the actual workspace manifest"
    );

    let events = fixture.a.events(&EventQuery::default())?;
    let decision = events
        .iter()
        .find(|event| event.seq == worker.receipt.audit_seq)
        .context("the decision audit event is absent")?;
    let admitted = events
        .iter()
        .find(|event| event.seq == admission.audit_seq)
        .context("the admission audit event is absent")?;
    let admission_data = &admitted.event.data;
    let supervised_request: SupervisedAdmissionV1 =
        serde_json::from_value(admission_data["input"]["request"].clone())
            .map_err(|_| anyhow::anyhow!("the supervised admission audit request is invalid"))?;
    let worker_record = &admission_data["worker_record"];
    ensure!(
        fixture.a.integrity()?.valid
            && decision.seq < admitted.seq
            && decision.hash == worker.receipt.audit_hash
            && decision.event.kind == "security.decision"
            && decision.event.session_id.as_deref() == Some(worker.launch.session_id.as_str())
            && decision
                .event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Allow)
            && admitted.hash == admission.audit_hash
            && admitted.event.kind == "control.supervision.transition"
            && admitted.event.source == "gateway"
            && admitted.event.provenance == "observed"
            && admitted.event.session_id.as_deref() == Some(worker.launch.session_id.as_str())
            && admission_data["version"] == 2
            && admission_data["input"]["action"] == "admission"
            && supervised_request.intent_hash == intent_hash
            && supervised_request.admission.operation_id == operation_id
            && supervised_request.input_hash()? == admission.input_hash
            && worker_record["action"] == "admit"
            && worker_record["operation_id"] == operation_id
            && worker_record["input_hash"] == supervised_request.admission.input_hash()?
            && worker_record["worker"] == serde_json::to_value(worker)?,
        "the recorded admission does not follow the matching Allow decision"
    );
    ensure!(
        fixture.a.worker_usage()?.is_empty()
            && !events
                .iter()
                .any(|event| event.event.kind == "command.started")
            && fs::read(&original)?.as_slice() == PROJECT_CONTENT
            && fs::read_dir(project)?.count() == 1,
        "a provider start, allocation or source change escaped the admission-only boundary"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_submit_codex_api_commits_admission_before_runtime_mutation() -> Result<()> {
    let fixture = Fixture::new_codex_admission(90).await?;
    eprintln!(
        "retained synthetic admission evidence: {}",
        fixture.root.display()
    );
    let result = verify_admission(&fixture).await;
    let cleanup = fixture.close().await;
    if cleanup.is_ok() {
        eprintln!("synthetic admission fixture cleanup confirmed");
    }
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("fixture cleanup failed: {cleanup}")))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
