use agentsandbox::{
    audit::Store,
    model::{Decision, DecisionRequest, Event, Outcome},
    mysql_storage::audit::MysqlAuditStore,
    policy::request_hash,
    redaction::Redactor,
};
use anyhow::Context;
use mysql_async::Conn;
use serde_json::json;
use std::{fs::Permissions, os::unix::fs::PermissionsExt};

pub(super) async fn exercise(
    bootstrapper: &MysqlAuditStore,
    writer: &MysqlAuditStore,
    mut inspector: Conn,
) -> anyhow::Result<()> {
    bootstrapper.initialize_supervised().await?;
    let directory = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir()?;
    let sqlite = Store::open(
        &directory.path().canonicalize()?.join("audit.sqlite"),
        Redactor::new(Vec::new()),
        128,
    )?;
    let mut observed_child = Event::new("agent.started");
    observed_child.agent_id = Some("agent-observed-child".into());
    observed_child.parent_agent_id = Some("agent-z".into());
    for event in super::entities::facts().into_iter().chain([
        observed_child,
        Event::new("command.requested"),
        Event::new("Security.Warning"),
    ]) {
        sqlite.append(event.clone())?;
        writer.append(event).await?;
    }

    let review = super::decision::request("review", &["/usr/bin/id"]);
    let pending = super::decision::request("review", &["/usr/bin/id"]);
    let deny = super::decision::request("strict", &["/usr/bin/id"]);
    append_decision(writer, &sqlite, &review, Outcome::Review).await?;
    append_decision(writer, &sqlite, &pending, Outcome::Review).await?;
    let mut approval = review.clone();
    approval.human_approved = true;
    approval.approval_hash = Some(request_hash(&review));
    append_decision(writer, &sqlite, &approval, Outcome::Allow).await?;
    append_decision(writer, &sqlite, &deny, Outcome::Deny).await?;

    let summary = writer.overview().await?;
    assert_eq!(
        summary["counts"],
        json!({
            "agents": 2,
            "subagents": 1,
            "declared_agents": 2,
            "declared_subagents": 1,
            "sandboxes": 2,
            "commands": 1,
            "allowed": 1,
            "denied": 1,
            "pending": 1,
            "security": 5,
        })
    );
    let local = sqlite.overview()?;
    for key in ["counts", "health", "resources", "coverage"] {
        assert_eq!(summary[key], local[key], "{key}");
    }
    let before = writer.verify().await?.seq;
    assert_eq!(summary["sequence"], before);

    // Legacy SQLite reviews without an ID each count as pending.
    for data in [json!({}), json!({"request_id": null})] {
        let mut legacy = Event::new("security.decision");
        legacy.data = data;
        legacy.decision = Some(Decision::new(Outcome::Review, "qa", "Legacy review"));
        sqlite.append(legacy)?;
    }
    assert_eq!(sqlite.overview()?["counts"]["pending"], 3);

    // The counts and the reported head must agree during a sibling append.
    let (concurrent, committed) = tokio::join!(
        writer.overview(),
        bootstrapper.append(Event::new("command.requested")),
    );
    let committed = committed?;
    let concurrent = concurrent?;
    let sequence = concurrent["sequence"]
        .as_i64()
        .context("missing sequence")?;
    assert_eq!(committed.seq, before + 1);
    assert!(sequence == before || sequence == committed.seq);
    assert_eq!(concurrent["counts"]["commands"], 1 + sequence - before);
    let current = writer.overview().await?;
    assert_eq!(current["sequence"], committed.seq);
    assert_eq!(current["counts"]["commands"], 2);

    let bad_hash = if committed.hash.starts_with('0') {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    super::integrity::replace_hash(&mut inspector, committed.seq, &bad_hash).await?;
    let error = writer.overview().await;
    super::integrity::replace_hash(&mut inspector, committed.seq, &committed.hash).await?;
    assert_eq!(error.unwrap_err().to_string(), "MySQL audit chain mismatch");
    assert_eq!(writer.overview().await?["counts"], current["counts"]);
    Ok(())
}

async fn append_decision(
    writer: &MysqlAuditStore,
    sqlite: &Store,
    request: &DecisionRequest,
    outcome: Outcome,
) -> anyhow::Result<()> {
    let proposal = (
        Decision::new(outcome, "qa", "Overview fixture"),
        json!(null),
    );
    sqlite
        .decision(request, Some(proposal.clone()))?
        .context("SQLite decision receipt missing")?;
    writer
        .decision(request, Some(proposal))
        .await?
        .context("MySQL decision receipt missing")?;
    Ok(())
}
