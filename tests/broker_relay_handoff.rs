//! Software composition tests, not engine or provider qualification.
//! Real service IPC, durable SQLite Core, workload CLI, owner, runtime exec port
//! and helper are used. Only engine observations and an empty workspace are
//! supplied by the explicitly selected std-only runtime witness.
#![cfg(unix)]

#[path = "support/broker_relay_handoff.rs"]
mod harness;

use agentsandbox::{
    broker::{
        clock::{HostClock, NativeHostClock},
        service::{self, protocol::*},
    },
    control::{HostClockSampleV1, WorkerStateV1},
    relay::{
        RelayAckV1, RelaySendV1,
        admission::{RelayChannelResultV1, RelayChannelStateV1},
        channel::{
            CloseReasonV1, ErrorCodeV1, InboxRequestV1, ReplyV1, RequestPayloadV1, ResponseResultV1,
        },
    },
};
use anyhow::{Context, Result, bail, ensure};
use harness::{
    A_RUNNER, B_RUNNER, Fixture, RelayAttempt, exchange, grant, relay_attempt, worker_terminal,
};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

#[test]
fn software_runtime_witness_refuses_contradictory_create_flags_before_mutation() -> Result<()> {
    harness::contradictory_create_flags_are_refused_before_fixture_mutation()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_broker_started_late_issue_two_endpoints_share_tenant_io() -> Result<()> {
    let fixture = Fixture::new(90).await?;
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

    let permission = grant(&fixture, source.clone(), destination.clone()).await?;
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
    fixture.close().await
}

fn authority_refusal(attempt: RelayAttempt) -> Result<()> {
    ensure!(
        matches!(
            attempt,
            RelayAttempt::Closed(None | Some(CloseReasonV1::IdentityChanged))
                | RelayAttempt::Response(ResponseResultV1::Error(
                    ErrorCodeV1::Authentication | ErrorCodeV1::Forbidden
                ))
        ),
        "revoked Ready owner must refuse or close for authority loss"
    );
    Ok(())
}

fn original_arm_still_live(fixture: &Fixture, job: Uuid) -> Result<()> {
    let fact = fixture.latest_fact(job)?;
    let original: HostClockSampleV1 = serde_json::from_value(fact["original_clock"].clone())?;
    let deadline: u128 = fact["original_deadline_ns_decimal"]
        .as_str()
        .context("original Arm deadline")?
        .parse()?;
    let current = NativeHostClock::new()?.sample()?;
    ensure!(
        current.method == original.method
            && current.boot_id == original.boot_id
            && current.continuous_ns_decimal.parse::<u128>()? < deadline,
        "authority-refusal oracle cannot be satisfied by Arm expiry"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_broker_ready_binding_revocation_refuses_helper_send_and_ack_before_dispatch()
-> Result<()> {
    let fixture = Fixture::new(90).await?;
    let source_job = fixture.submit("a", 90).await?;
    let destination_job = fixture.submit("a", 90).await?;
    let source = fixture.wait_endpoint(source_job).await?;
    let destination = fixture.wait_endpoint(destination_job).await?;
    let source_credential = fixture.issue(0, &source).await?;
    let destination_credential = fixture.issue(1, &destination).await?;
    fixture.ready(source_job).await?;
    fixture.ready(destination_job).await?;
    ensure!(
        fixture.exec_count().await == 2 && fixture.committed_claims()? == 2,
        "this parent requires two actual Ready owners, not lost Fresh claims"
    );
    let permission = grant(&fixture, source.clone(), destination.clone()).await?;
    let (mut sender, send_channel, send_epoch) = fixture.helper_connection(&source).await?;
    let (mut receiver, inbox_channel, inbox_epoch) =
        fixture.helper_connection(&destination).await?;
    let seed = RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: permission.resource_id,
        ttl_seconds: 60,
        body: r#"{"message":"unacknowledged public fixture"}"#.into(),
    };
    ensure!(
        matches!(
            exchange(
                &mut sender,
                &send_channel,
                send_epoch,
                RequestPayloadV1::Send(seed.clone())
            )
            .await?,
            ResponseResultV1::Ok(ReplyV1::Send(_))
        ),
        "positive send control must traverse the Ready owner"
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
        _ => bail!("positive inbox control must traverse the Ready recipient owner"),
    };
    ensure!(
        inbox.items.len() == 1
            && inbox.items[0].metadata.message_id == seed.operation_id
            && !inbox.items[0].metadata.acknowledged,
        "one existing unacknowledged message"
    );
    let effects = fixture.relay_effects()?;
    ensure!(
        effects.0 == [(seed.operation_id.clone(), false)],
        "Store positive-control message"
    );
    let sends_before = fixture.faults.sends.load(Ordering::SeqCst);
    let acks_before = fixture.faults.acks.load(Ordering::SeqCst);
    ensure!(
        sends_before == 1 && acks_before == 0,
        "HTTP business-route counters are live"
    );

    fixture.revoke(&source_credential).await?;
    original_arm_still_live(&fixture, source_job)?;
    let checks_before = fixture.faults.claims.load(Ordering::SeqCst);
    let denied_send = RelaySendV1 {
        operation_id: Uuid::new_v4().to_string(),
        ..seed.clone()
    };
    authority_refusal(
        relay_attempt(
            &mut sender,
            &send_channel,
            send_epoch,
            RequestPayloadV1::Send(denied_send.clone()),
        )
        .await?,
    )?;
    original_arm_still_live(&fixture, source_job)?;
    ensure!(
        fixture.faults.claims.load(Ordering::SeqCst) > checks_before,
        "request through the real helper must attempt current Core authority validation"
    );
    ensure!(
        fixture.faults.sends.load(Ordering::SeqCst) == sends_before,
        "revoked Ready request must stop before even a forbidden business HTTP dispatch"
    );
    fixture.assert_no_outbox_dispatch(&denied_send.operation_id)?;
    ensure!(
        fixture.relay_effects()? == effects,
        "revoked send cannot create a Store message/operation"
    );

    fixture.revoke(&destination_credential).await?;
    original_arm_still_live(&fixture, destination_job)?;
    let checks_before = fixture.faults.claims.load(Ordering::SeqCst);
    let denied_ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: seed.operation_id.clone(),
    };
    authority_refusal(
        relay_attempt(
            &mut receiver,
            &inbox_channel,
            inbox_epoch,
            RequestPayloadV1::Ack(denied_ack.clone()),
        )
        .await?,
    )?;
    original_arm_still_live(&fixture, destination_job)?;
    ensure!(
        fixture.faults.claims.load(Ordering::SeqCst) > checks_before,
        "recipient request must also revalidate current authority"
    );
    ensure!(
        fixture.faults.acks.load(Ordering::SeqCst) == acks_before,
        "revoked recipient cannot dispatch even a forbidden ACK HTTP request"
    );
    fixture.assert_no_outbox_dispatch(&denied_ack.operation_id)?;
    ensure!(
        fixture.relay_effects()? == effects,
        "existing message stays unacknowledged and neither denied operation enters Store"
    );
    ensure!(
        fixture.exec_count().await == 2 && fixture.committed_claims()? == 2,
        "revocation cannot remint an exec or a fresh Core claim"
    );
    for endpoint in [&source, &destination] {
        let worker = fixture
            .a
            .worker(&endpoint.worker_id)?
            .context("worker after revoked request")?;
        ensure!(
            worker.state == WorkerStateV1::Running && worker.quota_held,
            "relay closure cannot masquerade as worker terminal/quota release"
        );
    }
    drop(sender);
    drop(receiver);
    fixture.cancel("a", source_job).await?;
    fixture.cancel("a", destination_job).await?;
    worker_terminal(&fixture.a, &source).await?;
    worker_terminal(&fixture.a, &destination).await?;
    // Current binding loss can retain Unknown channel cleanup. Never invent a
    // Core revision or discard the original records to make this test close.
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_broker_lost_claim_recovery_retains_history_without_exec() -> Result<()> {
    let mut fixture = Fixture::new(90).await?;
    let job = fixture.submit("a", 90).await?;
    let endpoint = fixture.wait_endpoint(job).await?;
    fixture.faults.lose_claim.store(true, Ordering::SeqCst);
    let credential = fixture.issue(0, &endpoint).await?;
    timeout(Duration::from_secs(15), async {
        loop {
            if fixture.faults.lost.load(Ordering::SeqCst) == 1 {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    ensure!(
        fixture.committed_claims()? == 1 && fixture.exec_count().await == 0,
        "lost fresh response leaves an accepted durable claim, without an exec capability"
    );
    let fact = fixture.latest_fact(job)?;
    let channel = fact["claim"]["channel_id"]
        .as_str()
        .context("durable claim channel")?;
    let claim = fact["claim"].clone();
    let url = format!(
        "{}/control/api/v1/relay/channels/{channel}/claims",
        fixture.origin
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let replay = client
        .post(&url)
        .bearer_auth(A_RUNNER)
        .json(&claim)
        .send()
        .await?;
    ensure!(
        replay.status() == reqwest::StatusCode::OK,
        "historical claim returns HTTP 200"
    );
    let replay: RelayChannelResultV1 = replay.json().await?;
    ensure!(
        replay.replayed
            && claim["operation_id"].as_str() == Some(replay.receipt.operation_id.as_str()),
        "claim replay retains the exact historical receipt"
    );
    ensure!(
        fixture.committed_claims()? == 1 && fixture.exec_count().await == 0,
        "historical replay cannot remint fresh exec authority"
    );

    // Owned broker-process crash; the real independent enforcer remains alive.
    // This does not assert that an old local exec process was joined.
    fixture.restart_broker().await?;
    let reply = service::request(
        &fixture.broker_root,
        "a",
        A_RUNNER,
        BrokerCommandV1::Resume {
            operation_id: job,
            command_id: Uuid::new_v4(),
        },
    )
    .await?;
    ensure!(
        matches!(reply, BrokerReplyV1::CommandAcknowledged { .. }),
        "durable WorkerClaim recovery must use the real Resume callback"
    );
    timeout(Duration::from_secs(15), async {
        loop {
            let status = fixture.status("a", job).await?;
            if status.phase == Some(RelayAttachmentPhaseV1::Unknown) {
                ensure!(
                    status.reconciliation_required,
                    "unknown outcome remains explicit"
                );
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    ensure!(
        fixture.committed_claims()? == 1 && fixture.exec_count().await == 0,
        "durable recovery cannot execute a replayed claim"
    );
    fixture.revoke(&credential).await?;
    let refused = client
        .post(&url)
        .bearer_auth(A_RUNNER)
        .json(&claim)
        .send()
        .await?;
    ensure!(
        refused.status() == reqwest::StatusCode::FORBIDDEN,
        "current claim replay is refused after real workload revocation"
    );
    ensure!(
        fixture.committed_claims()? == 1 && fixture.exec_count().await == 0,
        "revocation cannot create a second effect"
    );
    fixture.cancel("a", job).await?;
    worker_terminal(&fixture.a, &endpoint).await?;
    // Cleanup after a lost response/recovery can remain Unknown: no fabricated
    // Core revision or old-process join is supplied by this fixture.
    ensure!(
        fixture.latest_fact(job)?["claim"] == claim,
        "unknown claim identity survives terminal worker observation"
    );
    fixture.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_broker_scoped_relay_ipc_refuses_before_private_detail() -> Result<()> {
    let fixture = Fixture::new(90).await?;
    let result = verify_scoped_relay_ipc(&fixture).await;
    let cleanup = fixture.close().await;
    result.and(cleanup)
}

async fn verify_scoped_relay_ipc(fixture: &Fixture) -> Result<()> {
    let a_job = fixture.submit("a", 90).await?;
    let a_endpoint = fixture.wait_endpoint(a_job).await?;
    let b_job = fixture.submit("b", 90).await?;
    let b_endpoint = fixture.wait_worker("b").await?;
    let own = service::request(
        &fixture.broker_root,
        "b",
        B_RUNNER,
        BrokerCommandV1::Status {
            operation_id: b_job,
        },
    )
    .await?;
    let BrokerReplyV1::Status { status } = own else {
        bail!("B cannot read its own historical Status V1");
    };
    ensure!(
        status.operation_id == b_job,
        "historical status remains scoped"
    );
    let value = serde_json::to_value(&status)?;
    ensure!(
        value.get("relay").is_none() && value.get("channel_id").is_none(),
        "RelayStatus must not change the historical Status V1 DTO"
    );
    let denied_profile = service::request(
        &fixture.broker_root,
        "a",
        B_RUNNER,
        BrokerCommandV1::RelayStatus {
            operation_id: a_job,
        },
    )
    .await?;
    ensure!(
        matches!(
            denied_profile,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::Forbidden
            }
        ),
        "B credential must be refused at the profile scope gate"
    );
    let denied_job = service::request(
        &fixture.broker_root,
        "b",
        B_RUNNER,
        BrokerCommandV1::RelayStatus {
            operation_id: a_job,
        },
    )
    .await?;
    ensure!(
        matches!(
            denied_job,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::InvalidInput
            }
        ),
        "B profile must refuse A UUID before loading relay detail"
    );
    let own_relay = fixture.status("a", a_job).await?;
    ensure!(
        own_relay.endpoint.as_ref() == Some(&a_endpoint),
        "authorized A detail remains available"
    );
    ensure!(
        fixture.exec_count().await == 0 && fixture.committed_claims()? == 0,
        "diagnostics confer no execution authority"
    );
    fixture.cancel("a", a_job).await?;
    fixture.cancel("b", b_job).await?;
    fixture.wait_worker_a_terminal(&a_endpoint).await?;
    worker_terminal(&fixture.b, &b_endpoint).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn software_broker_missing_receipt_expires_under_original_arm_without_exec() -> Result<()> {
    let fixture = Fixture::new(12).await?;
    let job = fixture.submit("a", 12).await?;
    let endpoint = fixture.wait_endpoint(job).await?;
    let fact = fixture.latest_fact(job)?;
    let original: HostClockSampleV1 = serde_json::from_value(fact["original_clock"].clone())?;
    let deadline: u128 = fact["original_deadline_ns_decimal"]
        .as_str()
        .context("original Arm absolute deadline")?
        .parse()?;
    let clock = NativeHostClock::new()?;
    let before = clock.sample()?;
    ensure!(
        before.method == original.method && before.boot_id == original.boot_id,
        "native clock must refer to the original Arm method and boot"
    );
    let before_ns: u128 = before.continuous_ns_decimal.parse()?;
    ensure!(
        before_ns < deadline,
        "fixture must reach Awaiting within its original budget"
    );
    worker_terminal(&fixture.a, &endpoint).await?;
    let after = clock.sample()?;
    let after_ns: u128 = after.continuous_ns_decimal.parse()?;
    ensure!(
        after.method == original.method
            && after.boot_id == original.boot_id
            && after_ns >= deadline
            && after_ns - deadline < 8_000_000_000,
        "missing receipt must stop under the original monotone deadline"
    );
    let final_fact = fixture.latest_fact(job)?;
    ensure!(
        final_fact["original_clock"] == fact["original_clock"]
            && final_fact["original_deadline_ns_decimal"] == fact["original_deadline_ns_decimal"],
        "polls and cleanup cannot renew the Arm"
    );
    ensure!(
        fixture.exec_count().await == 0 && fixture.committed_claims()? == 0,
        "absent issued material never authorizes exec"
    );
    ensure!(
        !fixture.operations[0].exists() && !fixture.operations[1].exists(),
        "the product does not auto-provision credentials while awaiting"
    );
    fixture.close().await
}
