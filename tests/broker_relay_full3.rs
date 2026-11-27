//! Product composition: real Full3 CLI, service IPC, Core HTTP/SQLite and helper.
//! Engine observations come only from the strict software witness.
#![cfg(unix)]
// The shared harness also serves the Fresh import gate (macOS); its entry
// points that this composition does not call are not dead product code.
#[allow(dead_code)]
#[path = "support/broker_relay_full3.rs"]
mod harness;

use agentsandbox::{
    control::WorkerStateV1,
    relay::{
        RelayAckV1, RelaySendV1,
        channel::{InboxRequestV1, ReplyV1, RequestPayloadV1, ResponseResultV1},
    },
};
use anyhow::{Context, Result, bail, ensure};
use harness::{
    Fixture, Role, exchange, grant, report_negative_assignment_progress, worker_terminal,
};
use uuid::Uuid;

#[test]
fn shared_software_runtime_refuses_contradictory_create_flags_before_mutation() -> Result<()> {
    harness::contradictory_create_flags_are_refused_before_fixture_mutation()
}

#[test]
fn full3_history_hash_checks_real_journal_frames_and_refuses_unknown_or_mismatched_heads()
-> Result<()> {
    harness::history_hash_checks_real_journal_frames()
}

#[test]
fn full3_timeout_import_outcomes_preserve_unknowns_without_private_text() -> Result<()> {
    harness::timeout_import_outcome_projection()
}

fn after_cleanup(exercise: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (exercise, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(_)) => Err(error.context("owned fixture cleanup also failed")),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_full3_restores_original_assignments_before_late_issue_two_endpoints_relay()
-> Result<()> {
    let mut fixture = Fixture::new_fresh().await?;
    let exercise = restored_pair(&mut fixture).await;
    let ticket = fixture.timeout_observation_ticket();
    // Report the oracle only after all owned service/guardian/tasks were joined.
    let cleanup = fixture.close().await;
    if let Some(ticket) = ticket {
        ticket.emit_after_cleanup(cleanup.is_ok());
    }
    after_cleanup(exercise, cleanup)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_full3_existing_reads_original_relay_assignment_without_authorizing_execution()
-> Result<()> {
    println!("full3_read_stage=fresh_requested");
    let mut fixture = Fixture::new_fresh().await?;
    println!("full3_read_stage=fresh_ready");
    let exercise = async {
        let job = fixture.submit("a", 90).await?;
        println!("full3_read_stage=submitted");
        fixture.wait_endpoint(job).await?;
        println!("full3_read_stage=endpoint_retained");
        let before = fixture.history(job, Role::FullA).await?;
        fixture.assert_no_relay_effects().await?;
        println!("full3_read_stage=history_confirmed");
        fixture.restart(Role::FullA).await?;
        println!("full3_read_stage=existing_ready");
        fixture.verify_retained_relay_assignment(job, &before).await
    }
    .await;
    let ticket = fixture.timeout_observation_ticket();
    let cleanup = fixture.close().await;
    if cleanup.is_ok() {
        println!("full3_read_cleanup=closed");
    } else {
        println!("full3_read_cleanup=unconfirmed");
    }
    if let Some(ticket) = ticket {
        ticket.emit_after_cleanup(cleanup.is_ok());
    }
    after_cleanup(exercise, cleanup)
}

async fn restored_pair(fixture: &mut Fixture) -> Result<()> {
    let first = fixture.submit("a", 90).await?;
    let second = fixture.submit("a", 90).await?;
    let source = fixture.wait_endpoint(first).await?;
    let destination = fixture.wait_endpoint(second).await?;
    ensure!(
        source != destination,
        "distinct admitted worker incarnations"
    );
    for endpoint in [&source, &destination] {
        let worker = fixture
            .a
            .worker(&endpoint.worker_id)?
            .context("running worker")?;
        ensure!(
            worker.state == WorkerStateV1::Running && worker.quota_held,
            "late issuance must start from the actual Running callback"
        );
    }
    let original_source = fixture.history(first, Role::FullA).await?;
    let original_destination = fixture.history(second, Role::FullA).await?;
    fixture.assert_no_relay_effects().await?;
    fixture.restart(Role::FullA).await?;
    // These are IPC replies from the NEW broker registry, not old fact files.
    // c3 must restore the exact assigned slot before it calls relay.started.
    ensure!(
        fixture.wait_endpoint(first).await? == source
            && fixture.wait_endpoint(second).await? == destination,
        "the new broker must restore both original endpoints before issue"
    );
    original_source.unchanged(&fixture.history(first, Role::FullA).await?)?;
    original_destination.unchanged(&fixture.history(second, Role::FullA).await?)?;
    fixture.assert_no_relay_effects().await?;
    ensure!(
        fixture.exec_count().await == 0 && fixture.committed_claims()? == 0,
        "missing issued references cannot authorize exec"
    );
    fixture.issue(0, &source).await?;
    fixture.issue(1, &destination).await?;
    fixture.ready(first).await?;
    fixture.ready(second).await?;
    ensure!(
        fixture.exec_count().await == 2 && fixture.committed_claims()? == 2,
        "each endpoint consumes one fresh Core claim and one exec"
    );
    fixture.tenant_directory()?;

    let permission = grant(fixture, source.clone(), destination.clone()).await?;
    let (mut sender, send_channel, send_epoch) = fixture.helper_connection(&source).await?;
    let (mut receiver, inbox_channel, inbox_epoch) =
        fixture.helper_connection(&destination).await?;
    let send = RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: permission.resource_id,
        ttl_seconds: 60,
        body: r#"{"message":"public software composition fixture"}"#.into(),
    };
    let first_receipt = match exchange(
        &mut sender,
        &send_channel,
        send_epoch,
        RequestPayloadV1::Send(send.clone()),
    )
    .await?
    {
        ResponseResultV1::Ok(ReplyV1::Send(receipt)) => receipt,
        _ => bail!("send through real helper/control/HTTP failed"),
    };
    ensure!(
        first_receipt.resource_id == send.operation_id,
        "send identity"
    );
    let record = fixture.durable_operation(&send.operation_id)?;
    ensure!(
        record["endpoint"] == serde_json::to_value(&source)?,
        "durable endpoint scope"
    );
    ensure!(
        record["request"]["operation"] == "send"
            && record["request"]["payload"]["body"] == send.body,
        "exact durable request content"
    );
    let inbox = match exchange(
        &mut receiver,
        &inbox_channel,
        inbox_epoch,
        RequestPayloadV1::Inbox(InboxRequestV1 {
            after: None,
            limit: 32,
        }),
    )
    .await?
    {
        ResponseResultV1::Ok(ReplyV1::Inbox(inbox)) => inbox,
        _ => bail!("inbox through real helper/control/HTTP failed"),
    };
    ensure!(inbox.items.len() == 1, "one durable message");
    ensure!(
        inbox.items[0].metadata.message_id == send.operation_id
            && inbox.items[0].metadata.source == source
            && inbox.items[0].metadata.destination == destination
            && inbox.items[0].body == send.body,
        "message content and directed scope"
    );
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: send.operation_id.clone(),
    };
    match exchange(
        &mut receiver,
        &inbox_channel,
        inbox_epoch,
        RequestPayloadV1::Ack(ack.clone()),
    )
    .await?
    {
        ResponseResultV1::Ok(ReplyV1::Ack(receipt)) => {
            ensure!(
                receipt.operation_id == ack.operation_id && receipt.resource_id == ack.message_id,
                "recipient ACK receipt"
            );
        }
        _ => bail!("ACK through real helper/control/HTTP failed"),
    }
    let record = fixture.durable_operation(&ack.operation_id)?;
    ensure!(
        record["endpoint"] == serde_json::to_value(&destination)?
            && record["request"]["operation"] == "ack",
        "durable ACK scope"
    );
    let replay = match exchange(
        &mut sender,
        &send_channel,
        send_epoch,
        RequestPayloadV1::Send(send.clone()),
    )
    .await?
    {
        ResponseResultV1::Ok(ReplyV1::Send(receipt)) => receipt,
        _ => bail!("exact retry through retained outbox failed"),
    };
    ensure!(
        replay.operation_id == first_receipt.operation_id
            && replay.resource_id == first_receipt.resource_id
            && replay.audit_seq == first_receipt.audit_seq
            && replay.audit_hash == first_receipt.audit_hash,
        "retry preserves the original effect receipt"
    );
    ensure!(
        fixture.exec_count().await == 2 && fixture.committed_claims()? == 2,
        "business retries cannot create another exec authority"
    );
    // Recheck the worker start/claim identities after late issue, Ready and
    // every send/inbox/ACK/replay; relay claim counters alone do not cover them.
    original_source.unchanged(&fixture.history(first, Role::FullA).await?)?;
    original_destination.unchanged(&fixture.history(second, Role::FullA).await?)?;
    drop(sender);
    drop(receiver);
    fixture.cancel("a", first).await?;
    fixture.cancel("a", second).await?;
    worker_terminal(&fixture.a, &source).await?;
    worker_terminal(&fixture.a, &destination).await?;
    // This requires the real acknowledged Observe operation ID from the broker
    // journal. Local EOF or fixture-process cleanup is insufficient.
    fixture.closed(first).await?;
    fixture.closed(second).await?;
    Ok(())
}

async fn refused_assignment(fixture: &mut Fixture, original: Role, selected: Role) -> Result<()> {
    let job = fixture.submit("a", 90).await?;
    report_negative_assignment_progress("awaiting_issued_workload", "submitted");
    // A legacy overflow is a FAILED prerequisite, never a fabricated history.
    let endpoint = fixture.wait_endpoint(job).await.context(
        "real Running/Awaiting prerequisite not reached; no history or capability fabricated",
    )?;
    report_negative_assignment_progress("original_history", "endpoint_retained");
    let before = fixture.history(job, original).await?;
    fixture.assert_no_relay_effects().await?;
    report_negative_assignment_progress("restart_existing", "original_history_confirmed");
    fixture.restart(selected).await?;
    report_negative_assignment_progress("original_arm_deadline", "existing_ready");
    before.require_live()?;
    report_negative_assignment_progress("resume", "original_arm_live");
    fixture.assert_assignment_refused(job).await?;
    report_negative_assignment_progress("recovered_history", "refusal_confirmed");
    before.unchanged(&fixture.recovered_history(job, original).await?)?;
    report_negative_assignment_progress("stop", "recovered_history_confirmed");
    // Return to the original genuine guardian for independent STOP.
    fixture.shutdown_jobs(&[job], original).await?;
    report_negative_assignment_progress("terminal", "stop_acknowledged");
    if let Err(error) = worker_terminal(&fixture.a, &endpoint).await {
        fixture.report_negative_terminal_wait(job, &endpoint, &before);
        return Err(error);
    }
    report_negative_assignment_progress("final_no_effects", "terminal_confirmed");
    fixture.assert_no_relay_effects().await?;
    report_negative_assignment_progress("done", "exercise_confirmed");
    Ok(())
}

async fn run_refused_assignment(
    setup: impl std::future::Future<Output = Result<Fixture>>,
    original: Role,
    selected: Role,
) -> Result<()> {
    use std::io::{Write as _, stderr};

    report_negative_assignment_progress("fixture_initialization", "none");
    let mut fixture = setup.await?;
    report_negative_assignment_progress("submit", "fixture_ready");
    let exercise = refused_assignment(&mut fixture, original, selected).await;
    let _ = writeln!(stderr().lock(), "\nfull3_negative_cleanup=started");
    let cleanup = fixture.close().await;
    let category = if cleanup.is_ok() {
        "closed"
    } else {
        "unconfirmed"
    };
    let _ = writeln!(stderr().lock(), "\nfull3_negative_cleanup={category}");
    after_cleanup(exercise, cleanup)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_full3_foreign_original_assignment_refuses_before_issue_exec_or_dispatch()
-> Result<()> {
    run_refused_assignment(Fixture::new_fresh(), Role::FullA, Role::FullB).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_full3_missing_assignment_refuses_genuine_legacy_history_before_issue_exec_or_dispatch()
-> Result<()> {
    run_refused_assignment(Fixture::new(Role::Legacy), Role::Legacy, Role::FullA).await
}
