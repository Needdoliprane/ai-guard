//! Real CLI, broker IPC and SQLite through a held synthetic import and worker completion.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[allow(dead_code, unused_imports)]
#[path = "support/broker_relay_full3.rs"]
mod harness;

use agentsandbox::{
    audit::EventQuery,
    broker::service::protocol::{
        BrokerReplyV1, JobResultOutcomeV1, JobStateV1, TerminalJobResultV1,
    },
    control::{
        ArmReceiptV1, HelperCleanupV1, HelperObservationV1, HelperStartClaimV1, MutationReceiptV1,
        RuntimeStatusV1, WorkerStateV1, WorkerV1,
    },
    control_fs::Directory,
    model::StoredEvent,
};
use anyhow::{Context, Result, bail, ensure};
use harness::{BIN, Fixture};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io, os::unix::process::ExitStatusExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, sleep, timeout, timeout_at},
};
use uuid::Uuid;

const CLI_STREAM_LIMIT: u64 = 64 * 1024;
// Harness observation bounds only, sized for a saturated shared host. Product
// deadlines, Arm timeouts and every acceptance oracle are unchanged.
const CLI_WAIT: Duration = Duration::from_secs(60);
const RELEASED_IMPORT_WAIT: Duration = Duration::from_secs(180);
const PLAYBACK_WAIT: Duration = Duration::from_secs(600);
const WORKER_WAIT: Duration = Duration::from_secs(300);

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
    let (status, stdout, stderr) = timeout(CLI_WAIT, async {
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

fn audit_event<'a>(
    events: &'a [StoredEvent],
    action: &str,
    session_id: &str,
    helper_id: Option<&str>,
) -> Result<&'a StoredEvent> {
    let mut matched = events.iter().filter(|entry| {
        let event = &entry.event;
        event.kind == "control.supervision.transition"
            && event.session_id.as_deref() == Some(session_id)
            && event.data["input"]["action"] == action
            && helper_id.is_none_or(|id| event.data["input"]["request"]["helper_id"] == id)
    });
    let entry = matched
        .next()
        .context("category=missing_sqlite_helper_event")?;
    ensure!(
        matched.next().is_none()
            && entry.event.source == "gateway"
            && entry.event.provenance == "observed",
        "category=unexpected_sqlite_helper_event"
    );
    Ok(entry)
}

fn audit_receipt(
    fixture: &Fixture,
    event: &StoredEvent,
    operation_id: &str,
    expected_input_hash: &str,
) -> Result<MutationReceiptV1> {
    let receipt = fixture
        .a
        .worker_operation(operation_id)
        .map_err(|_| anyhow::anyhow!("category=sqlite_receipt_read"))?
        .context("category=sqlite_receipt_missing")?;
    ensure!(
        receipt.operation_id == operation_id
            && receipt.input_hash == expected_input_hash
            && receipt.audit_seq == event.seq
            && receipt.audit_hash == event.hash,
        "category=sqlite_receipt_mismatch"
    );
    Ok(receipt)
}

async fn assert_held_cli(fixture: &Fixture, operation: Uuid) -> Result<()> {
    // Give both user commands a chance to report their own fixed categories.
    let (status, result) = tokio::join!(
        read_reply(broker_cli(fixture, "status", operation), "status"),
        read_reply(broker_cli(fixture, "result", operation), "result"),
    );
    for (name, observed) in [("status", &status), ("result", &result)] {
        match observed {
            Ok(_) => println!("phase=held_import cli={name} exit_code=0 category=typed_reply"),
            Err(error) => println!("phase=held_import {error}"),
        }
    }
    let BrokerReplyV1::Status { status } = status? else {
        bail!("phase=held_import category=unexpected_status_reply");
    };
    let BrokerReplyV1::Result { result } = result? else {
        bail!("phase=held_import category=unexpected_result_reply");
    };
    ensure!(
        status.operation_id == operation
            && status.state == JobStateV1::Accepted
            && status.worker_state == Some(WorkerStateV1::Preparing)
            && status.quota_held == Some(true)
            && status.executor_task_running
            && !status.cancel_requested
            && status.executor_error.is_none(),
        "phase=held_import category=unexpected_status_projection"
    );
    ensure!(
        result.version == 1
            && result.operation_id == operation
            && !result.cancel_requested
            && matches!(result.outcome, JobResultOutcomeV1::Pending),
        "phase=held_import category=unexpected_result_projection"
    );
    println!("phase=held_import status=Preparing result=Pending");
    Ok(())
}

async fn assert_held_sqlite(fixture: &Fixture, operation: Uuid) -> Result<HelperStartClaimV1> {
    let workers = fixture
        .a
        .workers()
        .map_err(|_| anyhow::anyhow!("phase=held_import category=sqlite_worker_read"))?;
    let [worker] = workers.as_slice() else {
        bail!("phase=held_import category=unexpected_worker_count");
    };
    ensure!(
        worker.admission_operation_id == operation.to_string()
            && worker.state == WorkerStateV1::Preparing
            && worker.quota_held
            && worker.attempt.create_operation_id.is_none()
            && worker.attempt.start_operation_id.is_none(),
        "phase=held_import category=unexpected_sqlite_worker"
    );
    let events = fixture
        .a
        .events(&EventQuery::default())
        .map_err(|_| anyhow::anyhow!("phase=held_import category=sqlite_event_read"))?;
    let claim = audit_event(&events, "helper_claim", &worker.launch.session_id, None)?;
    let claim: HelperStartClaimV1 =
        serde_json::from_value(claim.event.data["input"]["request"].clone())
            .map_err(|_| anyhow::anyhow!("phase=held_import category=invalid_sqlite_claim"))?;
    ensure!(
        claim.arm_receipt.worker_id == worker.worker_id
            && !events.iter().any(|entry| {
                entry.event.data["input"]["action"] == "helper_observation"
                    && entry.event.data["input"]["request"]["helper_id"] == claim.helper_id
            }),
        "phase=held_import category=premature_observation"
    );
    let counts = fixture.import_counts().await;
    ensure!(
        counts.imports_created == 1
            && counts.imports_removed == 0
            && counts.verification_created == 0
            && counts.workers_created == 0,
        "phase=held_import category=unexpected_fixture_counts"
    );
    println!("phase=held_import sqlite=Preparing counts={counts:?}");
    Ok(claim)
}

/// The closed batch failure diagnostic, when the real gateway stored one. It
/// carries fixed stage codes only, never an error chain or provider text.
fn batch_failure(fixture: &Fixture) -> Result<Option<Value>> {
    let events = fixture
        .a
        .events(&EventQuery::default())
        .map_err(|_| anyhow::anyhow!("phase=released_import category=sqlite_event_read"))?;
    Ok(events
        .iter()
        .find(|entry| entry.event.kind == "broker.batch.failure")
        .map(|entry| entry.event.data.clone()))
}

async fn after_import_release(
    fixture: &Fixture,
    claim: &HelperStartClaimV1,
) -> Result<[MutationReceiptV1; 2]> {
    // The executor starts verification only after the import observation and
    // removal were accepted and durably acknowledged by the broker. A batch
    // failure fact recorded by the real gateway ends the wait immediately.
    let counts = timeout(RELEASED_IMPORT_WAIT, async {
        loop {
            let counts = fixture.import_counts().await;
            if counts.verification_created > 0 {
                break Ok(counts);
            }
            if let Some(diagnostic) = batch_failure(fixture)? {
                break Err(anyhow::anyhow!(
                    "phase=released_import category=batch_failure_observed diagnostic={diagnostic}"
                ));
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("phase=released_import category=verification_not_reached")??;
    ensure!(
        counts.imports_created == 1 && counts.imports_removed == 1,
        "phase=released_import category=unexpected_fixture_counts"
    );
    let workers = fixture
        .a
        .workers()
        .map_err(|_| anyhow::anyhow!("phase=released_import category=sqlite_worker_read"))?;
    let worker = workers
        .iter()
        .find(|worker| worker.worker_id == claim.arm_receipt.worker_id)
        .context("phase=released_import category=sqlite_worker_missing")?;
    let events = fixture
        .a
        .events(&EventQuery::default())
        .map_err(|_| anyhow::anyhow!("phase=released_import category=sqlite_event_read"))?;
    let find = |action| {
        audit_event(
            &events,
            action,
            &worker.launch.session_id,
            Some(&claim.helper_id),
        )
    };
    let observation_event = find("helper_observation")?;
    let cleanup_event = find("helper_cleanup")?;
    let observation: HelperObservationV1 =
        serde_json::from_value(observation_event.event.data["input"]["request"].clone())
            .map_err(|_| anyhow::anyhow!("phase=released_import category=invalid_observation"))?;
    let cleanup: HelperCleanupV1 =
        serde_json::from_value(cleanup_event.event.data["input"]["request"].clone())
            .map_err(|_| anyhow::anyhow!("phase=released_import category=invalid_cleanup"))?;
    let arm = |receipt| {
        serde_json::to_value(receipt)
            .map_err(|_| anyhow::anyhow!("phase=released_import category=arm_receipt_encoding"))
    };
    let claim_arm = arm(&claim.arm_receipt)?;
    ensure!(
        arm(&observation.arm_receipt)? == claim_arm
            && arm(&cleanup.arm_receipt)? == claim_arm
            && observation.observation.status == RuntimeStatusV1::Exited
            && observation.observation.exit_code == Some(0)
            && observation.observation.running == Some(false)
            && observation.verification.is_none()
            && cleanup.removed
            && cleanup.error.is_none()
            && observation_event.seq < cleanup_event.seq,
        "phase=released_import category=unexpected_import_outcome"
    );
    let observation_hash = observation
        .input_hash()
        .map_err(|_| anyhow::anyhow!("phase=released_import category=observation_input_hash"))?;
    let cleanup_hash = cleanup
        .input_hash()
        .map_err(|_| anyhow::anyhow!("phase=released_import category=cleanup_input_hash"))?;
    let observation_receipt = audit_receipt(
        fixture,
        observation_event,
        &observation.operation_id,
        &observation_hash,
    )?;
    let cleanup_receipt =
        audit_receipt(fixture, cleanup_event, &cleanup.operation_id, &cleanup_hash)?;
    let integrity = fixture
        .a
        .integrity()
        .map_err(|_| anyhow::anyhow!("phase=released_import category=sqlite_integrity_read"))?;
    ensure!(
        integrity.valid,
        "phase=released_import category=sqlite_audit_integrity"
    );
    println!("phase=released_import observation=Exited cleanup=Removed counts={counts:?}");
    Ok([observation_receipt, cleanup_receipt])
}

async fn after_import_worker_terminal(
    fixture: &Fixture,
    operation: Uuid,
    import_arm: &ArmReceiptV1,
) -> Result<()> {
    // The extended gate waits for the same allocation that held quota during import.
    let running = timeout(WORKER_WAIT, async {
        loop {
            let worker = fixture
                .a
                .worker(&import_arm.worker_id)
                .map_err(|_| anyhow::anyhow!("phase=worker category=sqlite_worker_read"))?
                .context("phase=worker category=sqlite_worker_missing")?;
            if worker.state == WorkerStateV1::Running {
                break Ok::<_, anyhow::Error>(worker);
            }
            if let Some(diagnostic) = batch_failure(fixture)? {
                bail!("phase=worker category=batch_failure_observed diagnostic={diagnostic}");
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("phase=worker category=running_not_reached")??;
    let container_id = running
        .attempt
        .container_id
        .as_deref()
        .context("phase=worker category=runtime_identity_missing")?;
    ensure!(
        running.admission_operation_id == operation.to_string()
            && running.attempt.attempt_id == import_arm.attempt_id
            && running.observed_started_at.is_some()
            && running.quota_held,
        "phase=worker category=unexpected_running_worker"
    );
    println!("phase=worker state=Running quota_held=true");
    fixture
        .complete_running_worker(operation, &running.launch.session_id, container_id)
        .await
        .map_err(|_| anyhow::anyhow!("phase=worker category=synthetic_completion_refused"))?;

    let (worker, terminal) = timeout(WORKER_WAIT, async {
        loop {
            let worker = fixture
                .a
                .worker(&running.worker_id)
                .map_err(|_| anyhow::anyhow!("phase=result category=sqlite_worker_read"))?
                .context("phase=result category=sqlite_worker_missing")?;
            if worker.state == WorkerStateV1::Terminal && worker.cleanup_removed == Some(true) {
                let BrokerReplyV1::Result { result } =
                    read_reply(broker_cli(fixture, "result", operation), "result").await?
                else {
                    bail!("phase=result category=unexpected_result_reply");
                };
                ensure!(
                    result.version == 1
                        && result.operation_id == operation
                        && !result.cancel_requested,
                    "phase=result category=unexpected_result_identity"
                );
                match result.outcome {
                    JobResultOutcomeV1::Terminal { result } => break Ok((worker, result)),
                    JobResultOutcomeV1::Pending => {}
                    _ => bail!("phase=result category=unexpected_result_outcome"),
                }
            }
            if let Some(diagnostic) = batch_failure(fixture)? {
                bail!("phase=result category=batch_failure_observed diagnostic={diagnostic}");
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("phase=result category=terminal_not_reached")??;
    let observed = worker
        .last_observation
        .as_ref()
        .context("phase=result category=terminal_observation_missing")?;
    ensure!(
        worker.worker_id == running.worker_id
            && worker.attempt == running.attempt
            && worker.observed_started_at == running.observed_started_at
            && worker.launch.authority_id == running.launch.authority_id
            && !worker.quota_held
            && observed.inspection.container_id == container_id
            && observed.inspection.authority_id == worker.launch.authority_id
            && observed.status == RuntimeStatusV1::Exited
            && observed.running == Some(false)
            && observed.finished_at.is_some()
            && observed.exit_code == Some(0)
            && observed.oom_killed == Some(false)
            && terminal.session_id.to_string() == running.launch.session_id
            && terminal.container_id == container_id
            && terminal.volume == worker.attempt.volume_name
            && terminal.exit_code == 0
            && terminal.oom_killed == Some(false)
            && terminal.changed_files == 0
            && !terminal.output_captured,
        "phase=result category=unexpected_terminal_worker"
    );
    verify_terminal_audit(fixture, &worker, &terminal)?;
    println!("phase=worker state=Terminal quota_held=false releases=1");
    println!("phase=result outcome=Terminal audit=Confirmed");
    Ok(())
}

fn verify_terminal_audit(
    fixture: &Fixture,
    worker: &WorkerV1,
    terminal: &TerminalJobResultV1,
) -> Result<()> {
    let events = fixture
        .a
        .events(&EventQuery {
            order: Some("asc".into()),
            limit: Some(1000),
            ..EventQuery::default()
        })
        .map_err(|_| anyhow::anyhow!("phase=result category=sqlite_event_read"))?;
    ensure!(
        events.len() < 1000,
        "phase=result category=audit_incomplete"
    );
    let session = worker.launch.session_id.as_str();
    let at = |seq, kind: &str| {
        events
            .iter()
            .find(|entry| {
                entry.seq == seq
                    && entry.event.kind == kind
                    && entry.event.session_id.as_deref() == Some(session)
            })
            .map(|entry| &entry.event.data)
            .with_context(|| format!("phase=result category=missing_audit_event kind={kind}"))
    };
    let worker_id = worker.worker_id.as_str();
    let attempt_id = worker.attempt.attempt_id.as_str();
    let result = at(terminal.result_audit_seq, "broker.batch.result")?;
    let scan = at(terminal.scan_audit_seq, "broker.batch.scan")?;
    let cleanup = at(terminal.cleanup_audit_seq, "control.worker.transition")?;
    // This fixture uses the SQLite V6 worker record with a durable snapshot.
    let cleaned = &cleanup["worker"];
    ensure!(
        result["worker_id"] == worker_id
            && result["attempt_id"] == attempt_id
            && result["observation"]["id"] == terminal.container_id
            && result["observation"]["process"]["exit_code"] == terminal.exit_code
            && scan["worker_id"] == worker_id
            && scan["manifest_digest"] == terminal.manifest_digest
            && scan["changed_files"].as_u64() == u64::try_from(terminal.changed_files).ok()
            && cleanup["action"] == "cleanup_result"
            && cleaned["worker_id"] == worker_id
            && cleaned["attempt"]["attempt_id"] == attempt_id
            && cleaned["attempt"]["container_id"] == terminal.container_id
            && cleaned["cleanup_removed"] == true,
        "phase=result category=audit_tuple_mismatch"
    );
    let mut held = None;
    let mut release = None;
    for entry in events.iter().filter(|entry| {
        entry.event.kind == "control.worker.transition"
            && entry.event.session_id.as_deref() == Some(session)
            && entry.event.data["worker"]["worker_id"] == worker_id
            && entry.event.data["worker"]["attempt"]["attempt_id"] == attempt_id
    }) {
        let quota_held = entry.event.data["worker"]["quota_held"]
            .as_bool()
            .context("phase=quota category=invalid_audit_snapshot")?;
        if held == Some(true) && !quota_held {
            ensure!(
                release.replace(entry).is_none(),
                "phase=quota category=multiple_releases"
            );
        }
        held = Some(quota_held);
    }
    let release = release.context("phase=quota category=release_missing")?;
    let observation = &release.event.data["observation"];
    ensure!(
        held == Some(false)
            && release.seq < terminal.cleanup_audit_seq
            && release.event.data["action"] == "observe"
            && observation["inspection"]["container_id"] == terminal.container_id
            && observation["status"] == "exited",
        "phase=quota category=release_observation_mismatch"
    );
    Ok(())
}

fn assert_closed_ack(
    root: &Path,
    operation: Uuid,
    receipts: &[MutationReceiptV1; 2],
) -> Result<()> {
    // Close the only fixture writer before reading the immutable checkpoint.
    let namespace = root.join("b/journals/a/broker-journal-v1");
    let scopes = std::fs::read_dir(namespace)?.collect::<io::Result<Vec<_>>>()?;
    let [scope] = scopes.as_slice() else {
        bail!("category=unexpected_scope_count");
    };
    let job = Directory::open_existing(&scope.path().join("jobs").join(operation.to_string()))?;
    let current = job.read_private_file("current.json", 8192)?;
    let current: Value = serde_json::from_slice(current.bytes())?;
    let revision = current["head"]["revision"]
        .as_u64()
        .context("missing revision")?;
    let checkpoint = job.read_private_file(format!("rev-{revision:020}.json"), 32 * 1024 * 1024)?;
    let record: Value = serde_json::from_slice(checkpoint.bytes())?;
    let version = record["version"].as_u64().context("missing version")?;
    let mut digest = Sha256::new();
    digest.update(format!("ai-guard:broker-checkpoint:v{version}"));
    digest.update([0]);
    digest.update(checkpoint.bytes());
    let operation_id = operation.to_string();
    ensure!(
        current["operation_id"] == operation_id
            && record["operation_id"] == operation_id
            && record["revision"] == revision
            && current["head"]["hash"] == format!("{:x}", digest.finalize()),
        "category=checkpoint_correlation"
    );
    let broker = match version {
        3 => &record["payload"]["broker"],
        4 => &record["payload"]["lifecycle"]["broker"],
        5 => &record["payload"]["outbox"]["lifecycle"]["broker"],
        _ => bail!("category=checkpoint_version"),
    };
    let operations = broker["supervision"]["operations"]
        .as_array()
        .context("missing operations")?;
    for (kind, receipt) in ["helper_observation", "helper_cleanup"]
        .into_iter()
        .zip(receipts)
    {
        let encoded = serde_json::to_value(receipt)?;
        ensure!(
            operations.iter().any(|entry| {
                entry["kind"] == kind
                    && entry["request"]["operation_id"] == receipt.operation_id
                    && entry["response"] == encoded
            }),
            "category=checkpoint_missing_ack"
        );
    }
    Ok(())
}

struct Playback {
    cli: Result<()>,
    receipts: [MutationReceiptV1; 2],
    import_arm: ArmReceiptV1,
}

async fn exercise(fixture: &Fixture, operation: Uuid) -> Result<Playback> {
    let source = fixture.root.join("sources");
    let mut gate = fixture.hold_import().await?;
    let held: Result<_> = async {
        let mut submit = broker_cli(fixture, "submit", operation);
        submit
            .arg("--project")
            .arg(source)
            .args(["--timeout-seconds", "90", "--", "/usr/bin/id"]);
        let BrokerReplyV1::Accepted {
            operation_id: accepted_operation,
            replayed: false,
            provider_resolution: None,
            ..
        } = read_reply(submit, "submit").await?
        else {
            bail!("phase=submit category=unexpected_acceptance");
        };
        ensure!(
            operation == accepted_operation,
            "phase=submit category=unrelated_acceptance"
        );
        println!("phase=submit cli=submit exit_code=0 category=Accepted");
        gate.wait().await?;
        let claim = assert_held_sqlite(fixture, operation).await;
        let cli = assert_held_cli(fixture, operation).await;
        Ok((claim, cli))
    }
    .await;
    let release = gate.release();
    let (claim, cli) = held?;
    release?;
    let claim = claim?;
    let receipts = after_import_release(fixture, &claim).await?;
    Ok(Playback {
        cli,
        receipts,
        import_arm: claim.arm_receipt,
    })
}

#[test]
fn broker_import_witness_requires_exact_pull_policy() -> Result<()> {
    harness::contradictory_create_flags_are_refused_before_fixture_mutation()?;
    println!("phase=fixture_create_grammar category=closed_no_pull");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_import_result_pending_then_acknowledged() -> Result<()> {
    let fixture = Fixture::new_fresh().await?;
    let operation = Uuid::new_v4();
    let deadline = Instant::now() + PLAYBACK_WAIT;
    let result = timeout_at(deadline, exercise(&fixture, operation))
        .await
        .context("phase=playback category=fixture_deadline")
        .and_then(|result| result);
    let terminal = match &result {
        Ok(playback) if playback.cli.is_ok() => timeout_at(
            deadline,
            after_import_worker_terminal(&fixture, operation, &playback.import_arm),
        )
        .await
        .context("phase=worker category=fixture_deadline")
        .and_then(|result| result),
        _ => Ok(()),
    };
    let mut journal_ack = None;
    let (closed, witness_failures) = fixture
        .close_with_broker_stopped(|root| {
            if let Ok(playback) = &result {
                let checked = assert_closed_ack(root, operation, &playback.receipts)
                    .map_err(|_| anyhow::anyhow!("phase=closed_journal category=ack_not_proven"));
                if checked.is_ok() {
                    println!(
                        "phase=closed_journal observation_ack=Confirmed cleanup_ack=Confirmed"
                    );
                } else {
                    println!("phase=closed_journal category=ack_not_proven");
                }
                journal_ack = Some(checked);
            }
        })
        .await;
    println!(
        "phase=fixture_cleanup category={}",
        if closed.is_ok() {
            "children_reaped"
        } else {
            "not_confirmed"
        }
    );
    let playback = result?;
    playback.cli?;
    terminal?;
    journal_ack.context("phase=closed_journal category=ack_not_checked")??;
    ensure!(
        witness_failures == 0,
        "phase=fixture_witness category=errors_observed count={witness_failures}"
    );
    closed.map_err(|_| anyhow::anyhow!("phase=fixture_cleanup category=not_confirmed"))
}
