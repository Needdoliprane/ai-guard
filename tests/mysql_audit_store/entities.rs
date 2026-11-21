use agentsandbox::{
    audit::Store, model::Event, mysql_storage::audit::MysqlAuditStore, redaction::Redactor,
};
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
    assert!(writer.entities(false).await?.is_empty());
    assert!(writer.entities(true).await?.is_empty());
    for event in facts() {
        sqlite.append(event.clone())?;
        writer.append(event).await?;
    }

    let agents = writer.entities(false).await?;
    let sandboxes = writer.entities(true).await?;
    assert_eq!(agents, sqlite.entities(false)?);
    assert_eq!(sandboxes, sqlite.entities(true)?);
    assert_eq!(agents.len(), 4);
    assert_eq!(sandboxes.len(), 2);
    assert_eq!(agents[0]["id"], "agent-a");
    assert_eq!(agents[0]["status"], "unknown");
    assert_eq!(agents[1]["id"], "agent-z");
    assert_eq!(agents[1]["status"], "running");
    assert_eq!(agents[1]["model"], "observed-model");
    assert_eq!(agents[1]["field_provenance"]["model"], "observed");
    let root = agents
        .iter()
        .find(|agent| agent["provider_agent_id"] == "main")
        .expect("declared root");
    let child = agents
        .iter()
        .find(|agent| agent["provider_agent_id"] == "child")
        .expect("declared child");
    assert_eq!(child["parent_id"], root["id"]);
    assert_eq!(child["provenance"], "declared");
    assert_eq!(sandboxes[1]["engine"], "docker");
    assert_eq!(sandboxes[1]["resources"]["cpu_percent"], 12.5);

    // This later row has no entity. Its failure still refuses both lists.
    let later = writer.append(Event::new("qa.after.entities")).await?;
    let bad_hash = if later.hash.starts_with('0') {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    super::integrity::replace_hash(&mut inspector, later.seq, &bad_hash).await?;
    for sandboxes in [false, true] {
        assert_eq!(
            writer.entities(sandboxes).await.unwrap_err().to_string(),
            "MySQL audit chain mismatch"
        );
    }
    super::integrity::replace_hash(&mut inspector, later.seq, &later.hash).await?;
    assert_eq!(writer.entities(false).await?, agents);
    assert_eq!(writer.entities(true).await?, sandboxes);
    Ok(())
}

pub(super) fn facts() -> Vec<Event> {
    let mut started = Event::new("agent.started");
    started.agent_id = Some("agent-z".into());
    started.sandbox_id = Some("sandbox-z".into());
    started.session_id = Some("session-z".into());
    started.data = json!({"model":"observed-model"});
    let mut sandbox = Event::new("sandbox.created");
    sandbox.sandbox_id = started.sandbox_id.clone();
    sandbox.data = json!({"engine":"docker","mounts":["/workspace"]});
    let mut metrics = Event::new("sandbox.metrics");
    metrics.sandbox_id = started.sandbox_id.clone();
    metrics.data = json!({"cpu_percent":12.5});
    let mut claim = Event::new("command.failed");
    claim.agent_id = started.agent_id.clone();
    claim.sandbox_id = started.sandbox_id.clone();
    claim.provenance = "declared".into();
    claim.data = json!({"model":"claimed-model","engine":"claimed-engine"});

    let declarations = [("child", Some("main")), ("main", None)].map(|(id, parent)| {
        let mut event = Event::new("agent.declared");
        event.agent_id = started.agent_id.clone();
        event.sandbox_id = started.sandbox_id.clone();
        event.session_id = started.session_id.clone();
        event.provenance = "declared".into();
        event.source = "qa-provider".into();
        event.data = json!({
            "provider_agent_id": id,
            "provider_parent_agent_id": parent,
            "state": "started",
        });
        event
    });
    let [child, root] = declarations;
    let mut uncertain = Event::new("runtime.execution.error");
    uncertain.agent_id = Some("agent-a".into());
    uncertain.sandbox_id = Some("sandbox-a".into());
    vec![started, sandbox, metrics, claim, child, root, uncertain]
}
