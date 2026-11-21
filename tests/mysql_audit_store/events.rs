use agentsandbox::{
    audit::EventQuery,
    model::{Decision, Event, Outcome, StoredEvent},
    mysql_storage::audit::MysqlAuditStore,
};
use anyhow::Context;
use serde_json::json;

pub(super) async fn exercise(store: &MysqlAuditStore) -> anyhow::Result<()> {
    let marker = format!("qa-event-{}", uuid::Uuid::new_v4());
    let mut matching = Event::new("qa.events.search");
    matching.agent_id = Some("events-agent-a".into());
    matching.sandbox_id = Some("events-sandbox-a".into());
    matching.project_id = Some("events-project-a".into());
    matching.data = json!({"text": format!("{marker} literal%_ MiXeD")});
    let first = store.append(matching).await?;

    let mut wildcard_decoy = Event::new("qa.events.search");
    wildcard_decoy.agent_id = Some("events-agent-b".into());
    wildcard_decoy.sandbox_id = Some("events-sandbox-b".into());
    wildcard_decoy.project_id = Some("events-project-b".into());
    wildcard_decoy.data = json!({"text": format!("{marker} literalXX MiXeD")});
    let second = store.append(wildcard_decoy).await?;

    let mut timeline_one = Event::new("qa.events.timeline");
    timeline_one.data = json!({"step": 1, "fixture": marker});
    let timeline_first = store.append(timeline_one).await?;
    let mut timeline_two = Event::new("qa.events.timeline");
    timeline_two.data = json!({"step": 2, "fixture": marker});
    let timeline_second = store.append(timeline_two).await?;

    let mut decision_request = super::decision::request("strict", &["/usr/bin/id"]);
    decision_request.agent_id = Some("events-decision-agent".into());
    decision_request.sandbox_id = Some("events-decision-sandbox".into());
    decision_request.project_id = Some("events-decision-project".into());
    let decision = store
        .decision(
            &decision_request,
            Some((
                Decision::new(
                    Outcome::Allow,
                    "events_fixture",
                    "synthetic MySQL query fixture",
                ),
                json!({"fixture": marker}),
            )),
        )
        .await?
        .context("typed decision event was not stored")?;

    let head = store.head().await?;
    anyhow::ensure!(head == decision.seq && head > timeline_second.seq);

    let matching_q = format!("{} LITERAL%_ mixed", marker.to_ascii_uppercase());
    let literal_search = store
        .events(&EventQuery {
            q: Some(matching_q),
            order: Some("asc".into()),
            limit: Some(10),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(&literal_search, std::slice::from_ref(&first)),
        "search must be ASCII case insensitive and treat percent/underscore literally"
    );

    for (field, query) in [
        (
            "agent",
            EventQuery {
                agent_id: Some("events-agent-a".into()),
                ..Default::default()
            },
        ),
        (
            "sandbox",
            EventQuery {
                sandbox_id: Some("events-sandbox-a".into()),
                ..Default::default()
            },
        ),
        (
            "project",
            EventQuery {
                project_id: Some("events-project-a".into()),
                ..Default::default()
            },
        ),
    ] {
        let rows = store.events(&query).await?;
        anyhow::ensure!(
            same_ids(&rows, std::slice::from_ref(&first)),
            "{field} event filter returned the wrong rows"
        );
    }

    let decision_rows = store
        .events(&EventQuery {
            decision: Some("allow".into()),
            agent_id: Some("events-decision-agent".into()),
            sandbox_id: Some("events-decision-sandbox".into()),
            project_id: Some("events-decision-project".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(&decision_rows, std::slice::from_ref(&decision)),
        "decision filter did not return the matching typed decision"
    );
    let wrong_outcome = store
        .events(&EventQuery {
            decision: Some("deny".into()),
            agent_id: Some("events-decision-agent".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        wrong_outcome.is_empty(),
        "outcome mismatch returned a decision"
    );

    let ascending = store
        .events(&EventQuery {
            kind: Some("qa.events.timeline".into()),
            order: Some("asc".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(
            &ascending,
            &[timeline_first.clone(), timeline_second.clone()]
        ),
        "ascending sequence order is incorrect"
    );
    let descending = store
        .events(&EventQuery {
            kind: Some("qa.events.timeline".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(
            &descending,
            &[timeline_second.clone(), timeline_first.clone()]
        ),
        "default order must be descending"
    );
    let after = store
        .events(&EventQuery {
            kind: Some("qa.events.timeline".into()),
            after: Some(timeline_first.seq),
            order: Some("asc".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(same_ids(&after, std::slice::from_ref(&timeline_second)));
    let before = store
        .events(&EventQuery {
            kind: Some("qa.events.timeline".into()),
            before: Some(timeline_second.seq),
            order: Some("asc".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(same_ids(&before, std::slice::from_ref(&timeline_first)));

    let min_limit = store
        .events(&EventQuery {
            q: Some("literal%_".into()),
            limit: Some(0),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(min_limit.len() == 1, "zero limit must clamp to one row");
    let oversized_limit = store
        .events(&EventQuery {
            q: Some(marker.clone()),
            limit: Some(usize::MAX),
            order: Some("asc".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(
            &oversized_limit,
            &[
                first.clone(),
                second.clone(),
                timeline_first.clone(),
                timeline_second.clone(),
                decision.clone()
            ]
        ),
        "oversized limit must clamp safely while returning every matching event"
    );

    let offset = equivalent_offset(&first.received_at)?;
    let timed = store
        .events(&EventQuery {
            kind: Some("qa.events.search".into()),
            agent_id: Some("events-agent-a".into()),
            from: Some(offset.clone()),
            to: Some(offset),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        same_ids(&timed, std::slice::from_ref(&first)),
        "RFC3339 offset bounds lost the event"
    );
    let outside_sqlite_calendar = store
        .events(&EventQuery {
            to: Some("9999-12-31T23:30:00-01:00".into()),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        outside_sqlite_calendar.is_empty(),
        "UTC date outside the SQLite calendar produced an audit page"
    );
    let invalid_date = store
        .events(&EventQuery {
            from: Some("not a date".into()),
            ..Default::default()
        })
        .await;
    anyhow::ensure!(invalid_date.unwrap_err().to_string() == "invalid from date");

    let empty = store
        .events(&EventQuery {
            q: Some(format!("{} absent", marker)),
            after: Some(head),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(empty.is_empty(), "empty page unexpectedly contained events");

    let receipt_check = store
        .events(&EventQuery {
            q: Some(format!("{} literal%_", marker)),
            ..Default::default()
        })
        .await?;
    anyhow::ensure!(
        receipt_check.len() == 1
            && receipt_check[0].seq == first.seq
            && receipt_check[0].hash == first.hash
            && receipt_check[0].received_at == first.received_at,
        "queried event did not preserve its stored receipt, sequence and hash"
    );
    anyhow::ensure!(
        second.seq == first.seq + 1
            && timeline_first.seq == second.seq + 1
            && timeline_second.seq == timeline_first.seq + 1
            && decision.seq == timeline_second.seq + 1,
        "fixture writes did not receive a contiguous audit sequence"
    );
    Ok(())
}

fn same_ids(actual: &[StoredEvent], expected: &[StoredEvent]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.seq == expected.seq && actual.event.id == expected.event.id
        })
}

fn equivalent_offset(received_at: &str) -> anyhow::Result<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(received_at)?;
    let offset = chrono::FixedOffset::east_opt(2 * 60 * 60).context("invalid fixed test offset")?;
    Ok(parsed
        .with_timezone(&offset)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, false))
}
