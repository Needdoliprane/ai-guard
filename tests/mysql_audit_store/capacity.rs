use agentsandbox::{
    audit::{AuditCapacityFailureV1, AuditCapacityPolicyV1, EventQuery},
    model::{Decision, Event, Outcome, StoredEvent},
    mysql_storage::audit::MysqlAuditStore,
};
use mysql_async::{Conn, prelude::Queryable};
use serde_json::json;
use uuid::Uuid;

const PHYSICAL_BYTES: u64 = 268_435_456;

/// Exercises the default event reserves at a confirmed sequence-7 prefix.
pub(super) async fn exercise_events(store: &MysqlAuditStore) -> anyhow::Result<()> {
    let start = store.verify().await?;
    anyhow::ensure!(start.seq == 7, "capacity exercise expects append head 7");

    let event = Event::new("qa.capacity.ordinary.first");
    let first = store.append(event.clone()).await?;
    anyhow::ensure!(first.seq == 8, "ordinary write did not reach sequence 8");
    let head = store.verify().await?;
    let ordinary = store
        .append(Event::new("qa.capacity.ordinary.refused"))
        .await
        .err()
        .ok_or_else(|| {
            anyhow::anyhow!("ordinary write crossed the control and identity reserves")
        })?;
    anyhow::ensure!(
        ordinary.downcast_ref::<AuditCapacityFailureV1>()
            == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "ordinary reserve refusal was not typed: {ordinary}"
    );
    anyhow::ensure!(
        store.verify().await? == head && head.seq == 8,
        "ordinary refusal changed head"
    );

    let mut decisions = Vec::with_capacity(3);
    for index in 0..3 {
        let request = super::decision::request("strict", &["/usr/bin/id"]);
        let receipt = store
            .decision(
                &request,
                Some((
                    Decision::new(Outcome::Allow, "capacity_oracle", "reserved decision"),
                    json!({"capacity_index": index}),
                )),
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("new decision did not produce a receipt"))?;
        anyhow::ensure!(
            receipt.seq == 9 + index as i64,
            "decision sequence mismatch"
        );
        decisions.push((request, receipt));
    }
    let head = store.verify().await?;
    anyhow::ensure!(head.seq == 11, "three decisions did not reach sequence 11");

    let blocked_request = super::decision::request("strict", &["/usr/bin/true"]);
    let blocked = store
        .decision(
            &blocked_request,
            Some((
                Decision::new(Outcome::Deny, "capacity_oracle", "identity space protected"),
                json!({"capacity_index": "blocked"}),
            )),
        )
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("control consumed the final identity-reserved slot"))?;
    anyhow::ensure!(
        blocked.downcast_ref::<AuditCapacityFailureV1>()
            == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "identity reserve refusal was not typed: {blocked}"
    );
    anyhow::ensure!(
        store.verify().await? == head,
        "identity reserve refusal changed head"
    );

    let (last_request, last_receipt) = decisions
        .last()
        .ok_or_else(|| anyhow::anyhow!("decision receipt missing"))?;
    let replay = store
        .decision(
            last_request,
            Some((
                Decision::new(Outcome::Deny, "ignored_retry", "cached decision"),
                json!({"ignored_retry": true}),
            )),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("existing decision did not replay"))?;
    anyhow::ensure!(
        same_receipt(last_receipt, &replay),
        "decision replay changed its receipt"
    );
    anyhow::ensure!(
        same_receipt(&first, &store.append(event).await?),
        "ordinary replay changed its receipt after saturation"
    );
    anyhow::ensure!(store.verify().await? == head, "replay changed head");
    Ok(())
}

/// Shows byte admission below the physical byte ceiling with event slots left.
pub(super) async fn exercise_bytes(store: MysqlAuditStore) -> anyhow::Result<()> {
    let start = store.verify().await?;
    anyhow::ensure!(start.seq == 11, "byte exercise expects append head 11");
    let mut policy = AuditCapacityPolicyV1::resolved(128);
    policy.human_tenant.bytes = 2;
    policy.human_principal.bytes = 1;
    policy.control_reserve.bytes = PHYSICAL_BYTES
        .checked_sub(policy.identity_reserve.bytes)
        .and_then(|remaining| remaining.checked_sub(start.payload_bytes))
        .and_then(|remaining| remaining.checked_sub(100))
        .ok_or_else(|| anyhow::anyhow!("invalid remaining ordinary byte capacity"))?;
    anyhow::ensure!(
        PHYSICAL_BYTES
            - policy.control_reserve.bytes
            - policy.identity_reserve.bytes
            - start.payload_bytes
            == 100,
        "ordinary byte budget is not the requested 100 bytes"
    );
    let store = store.with_capacity_policy(policy)?;

    let mut ordinary_event = Event::new("qa.capacity.bytes.ordinary");
    ordinary_event.data = json!({"note": "x".repeat(256)});
    let failure = store.append(ordinary_event).await.err().ok_or_else(|| {
        anyhow::anyhow!("ordinary event exceeded neither logical nor physical bytes")
    })?;
    anyhow::ensure!(
        failure.downcast_ref::<AuditCapacityFailureV1>()
            == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "byte budget refusal was not typed: {failure}"
    );
    anyhow::ensure!(
        store.verify().await? == start,
        "byte refusal changed audit head"
    );
    anyhow::ensure!(
        start.seq < 128 && start.payload_bytes < PHYSICAL_BYTES,
        "ordinary refusal happened at a physical event or byte ceiling"
    );

    let request = super::decision::request("strict", &["/usr/bin/id"]);
    let receipt = store
        .decision(
            &request,
            Some((
                Decision::new(Outcome::Allow, "capacity_oracle", "decision reserve"),
                json!({"capacity_index": "byte-reserve"}),
            )),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("decision reserve did not admit a new decision"))?;
    anyhow::ensure!(receipt.seq == 12, "reserved decision sequence mismatch");
    let head = store.verify().await?;
    anyhow::ensure!(
        head.seq == 12 && head != start,
        "reserved decision did not advance head"
    );
    let replay = store
        .decision(
            &request,
            Some((
                Decision::new(Outcome::Deny, "ignored_retry", "cached decision"),
                json!({"ignored_retry": true}),
            )),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("reserved decision did not replay"))?;
    anyhow::ensure!(
        same_receipt(&receipt, &replay),
        "byte exercise replay changed receipt"
    );
    anyhow::ensure!(
        store.verify().await? == head,
        "decision replay changed audit head"
    );
    Ok(())
}

fn same_receipt(left: &StoredEvent, right: &StoredEvent) -> bool {
    left.seq == right.seq && left.hash == right.hash && left.event.id == right.event.id
}

pub(super) async fn exercise_control_delivery(
    bootstrapper: &MysqlAuditStore,
    ordinary: &MysqlAuditStore,
    mut inspector: Conn,
    open: impl Fn() -> anyhow::Result<MysqlAuditStore>,
) -> anyhow::Result<()> {
    let boundary = bootstrapper.initialize_supervised().await?;
    let before = ordinary.verify().await?;
    assert_eq!(before.seq, 2);
    assert_eq!(before.hash, boundary.hash);
    let original_rows = super::fixture_prefix(&mut inspector, before.seq).await?;

    // At 128 events, reserve exactly two events for control and guard the
    // rest. The prepared prefix leaves no ordinary event slot.
    let mut policy = AuditCapacityPolicyV1::resolved(128);
    policy.control_reserve.events = 2;
    policy.identity_reserve.events = 124;
    policy.human_tenant.events = 2;
    policy.human_principal.events = 1;
    let store = open()?.with_capacity_policy(policy.clone())?;

    let mut final_fact = Event::new("command.completed");
    final_fact.data = json!({"note": super::SECRET});
    let resume = resume_fact();
    assert_eq!(
        ordinary
            .append(resume.clone())
            .await
            .unwrap_err()
            .to_string(),
        "control events require typed mutation"
    );
    let error = store.append(final_fact.clone()).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );

    let mut masked_binding = resume.clone();
    masked_binding.data["identity_sha256"] = json!("a".repeat(64));
    let mut noncanonical = resume.clone();
    noncanonical.data["command_id"] = json!(Uuid::new_v4());
    for (invalid, expected) in [
        (
            Event::new("broker.retirement.final.unapproved"),
            "unsupported control delivery kind",
        ),
        (
            Event::new("command.requested"),
            "control delivery kind changed during preparation",
        ),
        (noncanonical, "noncanonical retirement resume request"),
        (masked_binding, "noncanonical retirement resume request"),
    ] {
        assert_eq!(
            store
                .append_control_event(invalid)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(store.verify().await?, before);
    }

    let final_receipt = store.append_control_event(final_fact.clone()).await?;
    assert_eq!(final_receipt.seq, before.seq + 1);
    assert_eq!(final_receipt.prev_hash, before.hash);
    assert!(!serde_json::to_string(&final_receipt)?.contains(super::SECRET));
    let resume_receipt = store.append_control_event(resume.clone()).await?;
    assert_eq!(resume_receipt.seq, final_receipt.seq + 1);
    assert_eq!(resume_receipt.prev_hash, final_receipt.hash);
    let after = store.verify().await?;
    let error = store
        .append_control_event(Event::new("command.completed"))
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(store.verify().await?, after);
    drop(store);

    let reopened = open()?.with_capacity_policy(policy)?;
    assert_eq!(reopened.verify().await?, after);
    assert!(!reopened.supervision_origin().await?.fresh_origin_retained);
    for (event, receipt) in [
        (final_fact, final_receipt),
        (resume.clone(), resume_receipt),
    ] {
        let replayed = reopened.append_control_event(event).await?;
        assert_eq!(
            serde_json::to_value(replayed)?,
            serde_json::to_value(receipt)?
        );
    }
    let mut conflicting_resume = resume;
    conflicting_resume.data["generation"] = json!(1);
    assert_eq!(
        reopened
            .append_control_event(conflicting_resume)
            .await
            .unwrap_err()
            .to_string(),
        "event id conflict"
    );
    assert_eq!(reopened.verify().await?, after);
    assert_eq!(
        super::fixture_prefix(&mut inspector, before.seq).await?,
        original_rows
    );
    let decision_count: Option<u64> = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        inspector.query_first("SELECT COUNT(*) FROM ai_guard_audit_decision"),
    )
    .await?
    .map_err(|_| anyhow::anyhow!("fixture decision index inspection failed"))?;
    assert_eq!(decision_count, Some(0));
    let page = reopened
        .events(&EventQuery {
            kind: Some("broker.retirement.resume.requested".into()),
            ..Default::default()
        })
        .await?;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].hash, after.hash);
    Ok(())
}

fn resume_fact() -> Event {
    let mut event = Event::new("broker.retirement.resume.requested");
    event.source = "ai-guard-broker".into();
    event.provenance = "observed".into();
    event.coverage = "native".into();
    event.data = json!({
        "version": 1,
        "operation_id": Uuid::new_v4(),
        "command_id": event.id,
        "retirement_id": Uuid::new_v4(),
        "arm_id": Uuid::new_v4(),
        "original_epoch": Uuid::new_v4(),
        "generation": 0,
        "identity_sha256": "d".repeat(64),
        "evidence_sha256": "b".repeat(64),
        "pending_ack_sha256": "c".repeat(64),
        "deadline_ns_decimal": "123456789012345"
    });
    event
}
